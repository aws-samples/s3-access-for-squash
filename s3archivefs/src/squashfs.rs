use std::io::{Error, ErrorKind};
use std::io::Write;
use std::time::Instant;
use std::mem::MaybeUninit;
use std::os::unix::fs::PermissionsExt;
use std::collections::HashMap;
use std::ffi::{CString, CStr, OsStr};
use std::os::unix::ffi::OsStrExt;
use log::{info, debug, error};
use libc::{c_void, c_int, off_t, size_t, lseek};
use nix::sys::stat::SFlag;
use crate::bindings::*;
use crate::repo::{CONTEXT, HoleDetectMode};
use super::*;

fn s_isreg(st_mode: sqfs_u16) -> bool {
    (SFlag::S_IFMT.bits() & st_mode as u32) == SFlag::S_IFREG.bits()
}

fn is_zero(buf: &[u8]) -> bool {
    let (prefix, aligned, suffix) = unsafe { buf.align_to::<u128>() };

    prefix.iter().all(|&x| x == 0)
        && suffix.iter().all(|&x| x == 0)
        && aligned.iter().all(|&x| x == 0)
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct sqfs_file_stdio_t {
    base: sqfs_file_t,
    readonly: c_int,
    size: sqfs_u64,
    fd: c_int,
}

pub extern "C" fn archive_read_at(base: *mut sqfs_file_t, offset: sqfs_u64,
        buffer: *mut c_void, size: usize) -> c_int {

    unsafe {

        debug!("read_at offset {}, size {}", offset, size);
        let (hdmode, meta_area) = CONTEXT.with(move |c| {
            let local = c.borrow();
            (
                local.as_ref().unwrap().hdmode(),
                local.as_ref().unwrap().is_metadata_area(offset as usize)
            )
        });

        if meta_area {
            debug!("read meta area {}", offset);
            return ((*base).write_at.unwrap())(base, offset, buffer, size);
        }

        let file = base as *mut sqfs_file_stdio_t;
        let fd: c_int = (*file).fd;

        let new_offset: off_t;
        if hdmode == HoleDetectMode::ALLZERO {
            let ret = ((*base).write_at.unwrap())(base, offset, buffer, size);
            if ret != 0 {
                return ret;
            }
            let data = std::slice::from_raw_parts(buffer as *const u8, size as usize);
            if is_zero(data) {
                new_offset = offset as off_t;
            } else {
                new_offset = offset as off_t + size as off_t;
            }
        } else {
            new_offset = lseek(fd, offset as off_t, libc::SEEK_HOLE);
            if new_offset < 0 {
                let errno: c_int =  Error::last_os_error().raw_os_error().unwrap().into();
                match errno {
                    libc::ENXIO => {
                        error!("lseek hole failed for offset {}, size: {}", offset, size);
                        return SQFS_ERROR_SQFS_ERROR_OUT_OF_BOUNDS;
                    },
                    _ => {
                        return SQFS_ERROR_SQFS_ERROR_IO;
                    }
                }
            }
        }

        debug!("got new offset {}", new_offset);
        if new_offset < (offset as off_t + size as off_t) {
            // load and write data
            let start_offset = new_offset as usize;
            let req_size = (offset as off_t + size as off_t - new_offset) as usize;

            debug!("start to read data from remote start_offset {}, size: {}", start_offset, req_size);
            // request data blocks from remote
            let res = CONTEXT.with(|c| {
                let local = c.borrow();
                local.as_ref().unwrap().request_remote_data_task(start_offset, req_size)
            });
            if res.is_err() {
                return SQFS_ERROR_SQFS_ERROR_IO;
            }
        }

        // it's actually read data
        return ((*base).write_at.unwrap())(base, offset, buffer, size);
    }
}

#[derive(Clone)]
pub struct Archive {
    file: *mut sqfs_file_t,
    sb: Box<sqfs_super_t>,
    cmp: *mut sqfs_compressor_t,
    xattr: *mut sqfs_xattr_reader_t,
    idtbl: *mut sqfs_id_table_t,
    dir: *mut sqfs_dir_reader_t,
    root: *mut sqfs_tree_node_t,
    data: *mut sqfs_data_reader_t,
}

impl Drop for Archive {
    fn drop(&mut self) {
        unsafe {
            sqfs_dir_tree_destroy(self.root)
        };
        sqfs_destroy(self.data);
        sqfs_destroy(self.dir);
        sqfs_destroy(self.idtbl);
        sqfs_destroy(self.xattr);
        sqfs_destroy(self.cmp);
        sqfs_destroy(self.file);
    }
}

impl Archive {
    pub fn new_from_sparse(path: &str, init_root: bool) -> Self {
        let f = CString::new(path).unwrap();
        let mut sb = MaybeUninit::<sqfs_super_t>::uninit();
        let mut cfg = MaybeUninit::<sqfs_compressor_config_t>::uninit();
        let mut cmp = MaybeUninit::<*mut sqfs_compressor_t>::uninit();
        let mut root = MaybeUninit::<*mut sqfs_tree_node_t>::uninit();
        unsafe {
            let file = sqfs_open_file(f.as_ptr(), SQFS_FILE_OPEN_FLAGS_SQFS_FILE_OPEN_READ_ONLY);
            if file.is_null() {
                panic!("can not open file {}", path);
            }

            let ret = sqfs_super_read(sb.as_mut_ptr(), file);
            if ret > 0 {
                panic!("error reading super block");
            }
            let sb = Box::new(sb.assume_init());

            sqfs_compressor_config_init(cfg.as_mut_ptr(),
                    sb.compression_id as u32,
                    sb.block_size as usize,
                    SQFS_COMP_FLAG_SQFS_COMP_FLAG_UNCOMPRESS as u16);


            let ret = sqfs_compressor_create(cfg.as_mut_ptr(), cmp.as_mut_ptr());
            if ret != 0 {
                panic!("error creating compressor");
            }

            let block_size = sb.block_size;
            let flags = sb.flags;
            let sb_p = Box::into_raw(sb);
            cfg.assume_init();
            let cmp = cmp.assume_init();

            let xattr: *mut sqfs_xattr_reader_t;
            if (flags & SQFS_SUPER_FLAGS_SQFS_FLAG_NO_XATTRS as u16) == 0 {
                xattr = sqfs_xattr_reader_create(0);
                if xattr.is_null() {
                    panic!("error creating xattr reader");
                }

                let ret = sqfs_xattr_reader_load(xattr, sb_p, file, cmp);
                if ret != 0 {
                    panic!("error loading xattr reader: {}", ret);
                }
            } else {
                xattr = std::ptr::null_mut::<sqfs_xattr_reader_t>();
            }

            let idtbl = sqfs_id_table_create(0);
            if idtbl.is_null() {
                panic!("error creating ID table");
            }

            let ret = sqfs_id_table_read(idtbl, file, sb_p, cmp);
            if ret > 1 {
                panic!("error loading ID table");
            }

            let dir = sqfs_dir_reader_create(sb_p, cmp, file, 0);
            if dir.is_null() {
                panic!("error creating directory reader");
            }

            if init_root {
                let ret = sqfs_dir_reader_get_full_hierarchy(dir, idtbl, std::ptr::null(), 0, root.as_mut_ptr());
                if ret != 0 {
                    panic!("error loading directory tree {}", ret);
                }
            } else {
                root.write(std::ptr::null_mut());
            }

            let root = root.assume_init();

            let data = sqfs_data_reader_create(file, block_size as usize, cmp, 0);
            if data.is_null() {
                panic!("error loading data reader");
            }

            let ret = sqfs_data_reader_load_fragment_table(data, sb_p);
            if ret != 0 {
                panic!("error loading fragment table {}", ret);
            }

            // as soon as init all struct, hook read_at
            type WriteAtType = unsafe extern "C" fn(*mut sqfs_file_t,sqfs_u64, *const c_void, usize) -> c_int;
            type ReadAtType = unsafe extern "C" fn(*mut sqfs_file_t,sqfs_u64, *mut c_void, usize) -> c_int;
            let read_at = (*file).read_at.replace(archive_read_at);
            let _ = (*file).write_at.replace(
                std::mem::transmute::<ReadAtType, WriteAtType>(read_at.unwrap())
            );

            info!("s3 archive fs init success");
            Self {
                file: file,
                sb: Box::from_raw(sb_p),
                cmp: cmp,
                xattr: xattr,
                idtbl: idtbl,
                dir: dir,
                root: root,
                data: data,
            }
        }
    }

    pub fn get_archive_file_size(&self) -> usize {

        let file = self.file as *mut sqfs_file_stdio_t;
        return unsafe {
            (*file).size as usize
        };
    }

    fn collect_xattrs(&self, inode: *const sqfs_inode_generic_t) -> Option<HashMap<Vec<u8>, Vec<u8>>> {

        if self.xattr.is_null() {
            return None;
        }

        let mut index = MaybeUninit::<sqfs_u32>::uninit();
        let mut desc = MaybeUninit::<sqfs_xattr_id_t>::uninit();
        unsafe {
            sqfs_inode_get_xattr_index(inode, index.as_mut_ptr());
            let index = index.assume_init();

            if index == 0xFFFFFFFF {
                return None;
            }

            let ret = sqfs_xattr_reader_get_desc(self.xattr, index, desc.as_mut_ptr());
            if ret != 0 {
                error!("error resolving xattr index");
                return None;
            }

            let ret = sqfs_xattr_reader_seek_kv(self.xattr, desc.as_mut_ptr());
            if ret != 0 {
                error!("error locating xattr KV pairs");
                return None;
            }

            let mut kv = HashMap::new();
            let desc = desc.assume_init();
            for _i in 0..desc.count {
                let mut key = MaybeUninit::<*mut sqfs_xattr_entry_t>::uninit();
                let mut val = MaybeUninit::<*mut sqfs_xattr_value_t>::uninit();

                let ret = sqfs_xattr_reader_read_key(self.xattr, key.as_mut_ptr());
                if ret != 0 {
                    error!("error reading xattr key");
                    return None;
                }

                let key = key.assume_init();
                let ret = sqfs_xattr_reader_read_value(self.xattr, key, val.as_mut_ptr());
                if ret != 0 {
                    error!("error reading xattr value");
                    return None;
                }
                let val = val.assume_init();

                let k = std::slice::from_raw_parts((*key).key.as_ptr() as *const u8, (*key).size as usize);
                let v = std::slice::from_raw_parts((*val).value.as_ptr() as *const u8, (*val).size as usize);

                kv.insert(Vec::from(k), Vec::from(v));

                sqfs_free(key as *mut c_void);
                sqfs_free(val as *mut c_void);
            }
            Some(kv)
        }
    }

    pub fn extract_one(&self, path: &str, outpath: &str) -> Result<usize, Error> {

        let f = CString::new(path).unwrap();
        let mut output = std::fs::File::create(outpath)?;

        let mut n = MaybeUninit::<*mut sqfs_tree_node_t>::uninit();
        let mut file_size = MaybeUninit::<sqfs_u64>::uninit();
        let filesz;
        let now = Instant::now();
        debug!("start to extract file {}", path);
        unsafe {
            let ret = sqfs_dir_reader_get_full_hierarchy(self.dir, self.idtbl, f.as_ptr(), 0, n.as_mut_ptr());
            if ret != 0 {
                if ret == SQFS_ERROR_SQFS_ERROR_NO_ENTRY {
                    println!("Entry not found");
                    return Err(Error::new(ErrorKind::NotFound, "Entry not found"));
                } else {
                    panic!("error loading directory tree {}", ret);
                }
            }
            let n = n.assume_init();
            let inode = (*n).inode;

            debug!("{:>6}: {:?}", "name", CStr::from_ptr((*n).name.as_ptr() as *const std::ffi::c_char).to_str().unwrap());
            debug!("{:>6}: {}", "uid", (*n).uid);
            debug!("{:>6}: {}", "gid", (*n).gid);
            debug!("{:>6}: {}", "mode", (*inode).base.mode);
            debug!("{:>6}: {}", "type", (*inode).base.type_);
            debug!("{:>6}: {}", "modt", (*inode).base.mod_time);
            debug!("{:>6}: {}", "ino", (*inode).base.inode_number);

            if !s_isreg((*inode).base.mode) {
                let s = format!("{} is not a regular file", path);
                info!("{}", &s);
                return Err(Error::new(ErrorKind::Other, s));
            }

            sqfs_inode_get_file_size(inode, file_size.as_mut_ptr());
            let mut file_size: size_t = file_size.assume_init() as size_t;
            debug!("{:>6}: {}", "size", file_size);
            filesz = file_size as usize;

            let blk_cnt = ((*inode).payload_bytes_used / std::mem::size_of::<sqfs_u32>() as u32) as usize;

            let mut i = 0;
            while i < blk_cnt {

                let mut chunk = MaybeUninit::<*mut sqfs_u8>::uninit();
                let mut chunk_size = MaybeUninit::<size_t>::uninit();
                let read: size_t;

                if (file_size) < (self.sb.block_size as size_t) {
                    read = file_size;
                } else {
                    read = self.sb.block_size as size_t;
                }
                
                let ret = sqfs_data_reader_get_block(self.data, inode, i, chunk_size.as_mut_ptr(), chunk.as_mut_ptr());
                if ret > 0 {
                    panic!("error reding data block: {}", ret);
                }
                let chunk = chunk.assume_init();
                let chunk_size = chunk_size.assume_init();
                let buf = std::slice::from_raw_parts(chunk, chunk_size);
                if let Err(e) = output.write(buf) {
                    libc::free(chunk as *mut c_void);
                    return Err(e);
                }

                let res = output.flush();
                libc::free(chunk as *mut c_void);
                if res.is_err() {
                    return Err(res.unwrap_err());
                }

                file_size -= read;
                i += 1;
            }

            if file_size > 0 {
                let mut chunk = MaybeUninit::<*mut sqfs_u8>::uninit();
                let mut chunk_size = MaybeUninit::<size_t>::uninit();

                debug!("file has {} fragmented", file_size);
                let ret = sqfs_data_reader_get_fragment(self.data, inode, chunk_size.as_mut_ptr(), chunk.as_mut_ptr());
                if ret > 0 {
                    panic!("error reding fragment block: {}", ret);
                }

                let chunk = chunk.assume_init();
                let chunk_size = chunk_size.assume_init();
                let buf = std::slice::from_raw_parts(chunk, chunk_size);
                if let Err(e) = output.write(buf) {
                    libc::free(chunk as *mut c_void);
                    return Err(e);
                }

                let res = output.flush();
                libc::free(chunk as *mut c_void);
                if res.is_err() {
                    return Err(res.unwrap_err());
                }
            }
            debug!("file content flushed to {}, cost: {:?}", outpath, now.elapsed());
            let now = Instant::now();

            // fill metadata for output file
            let meta = output.metadata()?;
            // set permission
            let mut perms = meta.permissions();
            perms.set_mode((*inode).base.mode as u32);
            std::fs::set_permissions(outpath, perms)?;
            // set last mod time
            filetime::set_file_mtime(outpath, filetime::FileTime::from_unix_time((*inode).base.mod_time as i64, 0))?;
            // set uid/gid
            if let Err(e) = file_owner::set_owner(outpath, file_owner::Owner::from_uid((*n).uid as u32)) {
                info!("failed to set uid, error: {}", e);
                // don't fail
                //return Err(Error::new(ErrorKind::Other, "failed to set uid"));
            }
            if let Err(e) = file_owner::set_group(outpath, file_owner::Group::from_gid((*n).gid as u32)) {
                info!("failed to set gid, error: {}", e);
                // don't fail
                //return Err(Error::new(ErrorKind::Other, "failed to set gid"));
            }
            // set xattr
            if let Some(hashmap) = self.collect_xattrs(inode) {
                for (k, v) in hashmap.iter() {
                    let key = OsStr::from_bytes(k);
                    xattr::set(outpath, key, &v)?;
                }
            } else {
                debug!("no xattr found");
            }
            debug!("write all metadata cost: {:?}", now.elapsed());
            info!("output file write to {}", outpath);
        }
        Ok(filesz)
    }

    pub fn get_sb(&self) -> sqfs_super_t {
        *self.sb.clone()
    }

    pub fn print_list(&self, path: Option<String>) {

        if path.is_none() {
            self.print_write_tree_dfs_root();
            return;
        }

        let f = CString::new(path.unwrap()).unwrap();

        let mut n = MaybeUninit::<*mut sqfs_tree_node_t>::uninit();
        unsafe {
            let ret = sqfs_dir_reader_get_full_hierarchy(self.dir, self.idtbl, f.as_ptr(), 0, n.as_mut_ptr());
            if ret != 0 {
                if ret == SQFS_ERROR_SQFS_ERROR_NO_ENTRY {
                    println!("Entry not found");
                    return;
                } else {
                    panic!("error loading directory tree {}", ret);
                }
            }
            let n = n.assume_init();
            self.print_write_tree_dfs(n, false);
        }
    }

    unsafe fn traverse_tree(mut n: *mut sqfs_tree_node_t, curr_path: &str, vec: &mut Vec<(String, libc::stat64)>) {

        /*
        if (*n).children.is_null() {
            let filepath = CStr::from_ptr((*n).name.as_ptr() as *const std::ffi::c_char).to_str().unwrap();
            let stat = Self::stat(n);
            vec.push((filepath.to_string(), stat));
            return;
        }
        */

        n = (*n).children;
        while !n.is_null() {

            let node_type = (*(*n).inode).base.type_ as u32;
            if node_type == SQFS_INODE_TYPE_SQFS_INODE_EXT_DIR || node_type == SQFS_INODE_TYPE_SQFS_INODE_DIR {
                let path = curr_path.to_owned() + "/" + CStr::from_ptr((*n).name.as_ptr() as *const std::ffi::c_char).to_str().unwrap();
                Self::traverse_tree(n, &path, vec);
            } else {
                let filepath = curr_path.to_owned() + "/" + CStr::from_ptr((*n).name.as_ptr() as *const std::ffi::c_char).to_str().unwrap();
                let stat = Self::stat(n);
                vec.push((filepath.to_string(), stat));
            }
            n = (*n).next;
        }
    }

    pub unsafe fn file_list(&self, path: Option<String>) -> Vec<(String, libc::stat64)> {

        let f;
        let curr_path;
        let _s;
        if path.is_none() {
            curr_path = "";
            f = CString::new(curr_path).unwrap();
        } else {
            _s = path.unwrap();
            curr_path = _s.as_str();
            f = CString::new(curr_path).unwrap();
        }

        let mut vec = Vec::new();

        let mut n = MaybeUninit::<*mut sqfs_tree_node_t>::uninit();
        let ret = sqfs_dir_reader_get_full_hierarchy(self.dir, self.idtbl, f.as_ptr(), 0, n.as_mut_ptr());
        if ret != 0 {
            if ret == SQFS_ERROR_SQFS_ERROR_NO_ENTRY {
                debug!("Entry not found");
                return vec;
            } else {
                panic!("error loading directory tree {}", ret);
            }
        }
        let n = n.assume_init();

        Self::traverse_tree(n, curr_path, &mut vec);
        vec
    }

    pub unsafe fn file_stat(&self, filepath: &str) -> Option<libc::stat64> {

        let f = CString::new(filepath).unwrap();

        let mut n = MaybeUninit::<*mut sqfs_tree_node_t>::uninit();
        let ret = sqfs_dir_reader_get_full_hierarchy(self.dir, self.idtbl, f.as_ptr(), 0, n.as_mut_ptr());
        if ret != 0 {
            if ret == SQFS_ERROR_SQFS_ERROR_NO_ENTRY {
                println!("Entry not found");
                return None;
            } else {
                panic!("error loading directory tree {}", ret);
            }
        }
        let n = n.assume_init();
        Some(Self::stat(n))
    }

    pub fn print_file_stat(&self, filepath: &str) {

        let f = CString::new(filepath).unwrap();

        let mut n = MaybeUninit::<*mut sqfs_tree_node_t>::uninit();
        unsafe {
            let ret = sqfs_dir_reader_get_full_hierarchy(self.dir, self.idtbl, f.as_ptr(), 0, n.as_mut_ptr());
            if ret != 0 {
                if ret == SQFS_ERROR_SQFS_ERROR_NO_ENTRY {
                    println!("Entry not found");
                    return;
                } else {
                    panic!("error loading directory tree {}", ret);
                }
            }
            let n = n.assume_init();
            self.print_stat(n);
        }
    }

    pub fn print_write_tree_dfs_root(&self) {
        if self.root.is_null() {
            println!("root hierarchy is not initialized");
            return;
        }
        self.print_write_tree_dfs(self.root, false);
    }

    pub fn print_write_tree_dfs(&self, mut n: *const sqfs_tree_node_t, print_stat: bool) {

        let mut p: *const sqfs_tree_node_t;
        let mut level;
        let mut mask;

        unsafe {

        n = (*n).children;
        while !n.is_null() {

            level = 0;
            mask = 0;

            p = (*n).parent;
            while !(*p).parent.is_null() {
                if !(*p).next.is_null() {
                    mask |= 1 << level;
                }
                level += 1;
                p = (*p).parent;
            }

            let mut i = level - 1;
            while i >= 0 {
                if (mask & (1 << i)) > 0 {
                    print!("|  ");
                } else {
                    print!("   ");
                }
                i -= 1;
            }

            if (*n).next.is_null() {
                print!("└─ ");
            } else {
                print!("├─ ");
            }
            print!("{:?}", CStr::from_ptr((*n).name.as_ptr() as *const std::ffi::c_char).to_str().unwrap());

            if (*(*n).inode).base.type_ == SQFS_INODE_TYPE_SQFS_INODE_SLINK as u16 {
                print!(" ⭢  {:?}", (*(*n).inode).extra);
            } else if (*(*n).inode).base.type_ == SQFS_INODE_TYPE_SQFS_INODE_EXT_SLINK as u16 {
                print!(" ⭢  {:?}", (*(*n).inode).extra);
            }

            println!("");
            self.print_write_tree_dfs(n, print_stat);
            if print_stat {
                self.print_stat(n);
            }
            n = (*n).next;
        }

        }
    }

    pub unsafe fn stat(n: *const sqfs_tree_node_t) -> libc::stat64 {

        let inode = (*n).inode;
        let st_dev: u64 = 0;
        let st_blksize: i64 = 0;
        let st_nlink: u64;
        let mut st_rdev: u64 = 0;
        let mut st_size: u64 = 0;
        let mut st_blocks: u64 = 0;
        let mut st_mode: u32 = (*inode).base.mode as u32;

        match (*inode).base.type_ as u32 {
            SQFS_INODE_TYPE_SQFS_INODE_FILE => {
                let mut size = MaybeUninit::<sqfs_u64>::uninit();
                sqfs_inode_get_file_size(inode, size.as_mut_ptr());
                st_size = size.assume_init();

                st_mode |= libc::S_IFREG;
                st_nlink = 1;
                if st_size == 0 {
                    st_blocks = 0;
                } else {
                    st_blocks = ((st_size - 1) >> 9) + 1;
                }
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_FILE => {
                let mut size = MaybeUninit::<sqfs_u64>::uninit();
                sqfs_inode_get_file_size(inode, size.as_mut_ptr());
                st_size = size.assume_init();

                st_mode |= libc::S_IFREG;
                st_nlink = (*inode).data.file_ext.nlink as u64;
                st_blocks = (st_size + (*inode).data.file_ext.sparse - 511) >> 9;
            },
            SQFS_INODE_TYPE_SQFS_INODE_DIR => {
                st_mode |= libc::S_IFDIR;
                st_nlink = (*inode).data.dir.nlink as u64;
                st_size = (*inode).data.dir.size as u64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_DIR => {
                st_mode |= libc::S_IFDIR;
                st_nlink = (*inode).data.dir_ext.nlink as u64;
                st_size = (*inode).data.dir_ext.size as u64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_FIFO => {
                st_mode |= libc::S_IFIFO;
                st_nlink = (*inode).data.ipc.nlink as u64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_FIFO => {
                st_mode |= libc::S_IFIFO;
                st_nlink = (*inode).data.ipc_ext.nlink as u64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_SOCKET => {
                st_mode |= libc::S_IFSOCK;
                st_nlink = (*inode).data.ipc.nlink as u64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_SOCKET => {
                st_mode |= libc::S_IFSOCK;
                st_nlink = (*inode).data.ipc_ext.nlink as u64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_BDEV => {
                st_mode |= libc::S_IFBLK;
                st_nlink = (*inode).data.dev.nlink as u64;
                st_rdev = (*inode).data.dev.devno as u64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_BDEV => {
                st_mode |= libc::S_IFBLK;
                st_nlink = (*inode).data.dev_ext.nlink as u64;
                st_rdev = (*inode).data.dev_ext.devno as u64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_CDEV => {
                st_mode |= libc::S_IFCHR;
                st_nlink = (*inode).data.dev.nlink as u64;
                st_rdev = (*inode).data.dev.devno as u64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_CDEV => {
                st_mode |= libc::S_IFCHR;
                st_nlink = (*inode).data.dev_ext.nlink as u64;
                st_rdev = (*inode).data.dev_ext.devno as u64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_SLINK => {
                st_mode |= libc::S_IFLNK;
                st_nlink = (*inode).data.slink.nlink as u64;
                st_size = (*inode).data.slink.target_size as u64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_SLINK => {
                st_mode |= libc::S_IFLNK;
                st_nlink = (*inode).data.slink_ext.nlink as u64;
                st_size = (*inode).data.slink_ext.target_size as u64;
            },
            _ => {
                panic!("unkown file type {}", (*inode).base.type_);
            },
        }

        let z_stat = MaybeUninit::<libc::stat64>::zeroed();
        let mut stat = z_stat.assume_init();
        stat.st_dev = st_dev;
        stat.st_ino = (*inode).base.inode_number as libc::ino64_t;
        stat.st_nlink = st_nlink as libc::nlink_t;
        stat.st_mode = st_mode;
        stat.st_uid = (*n).uid;
        stat.st_gid = (*n).gid;
        stat.st_rdev = st_rdev;
        stat.st_size = st_size as libc::off_t;
        stat.st_blksize = st_blksize as libc::blksize_t;
        stat.st_blocks = st_blocks as libc::blkcnt64_t;
        stat.st_atime = 0;
        stat.st_atime_nsec = 0;
        stat.st_mtime =  (*inode).base.mod_time as libc::time_t;
        stat.st_mtime_nsec = 0;
        stat.st_ctime = 0;
        stat.st_ctime_nsec = 0;

        stat
    }

    pub fn print_stat(&self, n: *const sqfs_tree_node_t) {

        unsafe {

        let inode = (*n).inode;

        println!("Name: {:?}", CStr::from_ptr((*n).name.as_ptr() as *const std::ffi::c_char).to_str().unwrap());
        println!("Inode type: {:?}", (*inode).base.type_);
        println!("Inode ino: {:?}", (*inode).base.inode_number);
        println!("UID: {} (index = {})", (*n).uid, (*inode).base.uid_idx);
        println!("GID: {} (index = {})", (*n).gid, (*inode).base.gid_idx);
        println!("Last modified: {}", (*inode).base.mod_time);

        match (*inode).base.type_ as u32 {
            SQFS_INODE_TYPE_SQFS_INODE_FILE | SQFS_INODE_TYPE_SQFS_INODE_EXT_FILE => {

                let mut location = MaybeUninit::<sqfs_u64>::uninit();
                sqfs_inode_get_file_block_start(inode, location.as_mut_ptr());
                let location = location.assume_init();

                let mut size = MaybeUninit::<sqfs_u64>::uninit();
                sqfs_inode_get_file_size(inode, size.as_mut_ptr());
                let size = size.assume_init();

                let mut frag_idx = MaybeUninit::<sqfs_u32>::uninit();
                let mut frag_offset = MaybeUninit::<sqfs_u32>::uninit();
                sqfs_inode_get_frag_location(inode, frag_idx.as_mut_ptr(), frag_offset.as_mut_ptr());
                let frag_idx = frag_idx.assume_init();
                let frag_offset = frag_offset.assume_init();

                println!("Fragment index: {}", frag_idx);
                println!("Fragment offset: {}", frag_offset);
                println!("File size: {}", size);

                if (*inode).base.type_ as u32 == SQFS_INODE_TYPE_SQFS_INODE_EXT_FILE {
                    println!("Sparse: {}", (*inode).data.file_ext.sparse);
                }

                println!("Blocks start: {}", location);
                let blk_cnt = ((*inode).payload_bytes_used / std::mem::size_of::<sqfs_u32>() as u32) as usize;
                println!("Blocks count: {}", blk_cnt);

                let mut i = 0;
                while i < blk_cnt {
                    let blk_sz = (*inode).extra.as_slice(blk_cnt)[i] & ((1 << 24) - 1);
                    let _flag = (*inode).extra.as_slice(blk_cnt)[i] & (1 << 24);
                    println!("\tBlock #{} size:{} ({})", i, blk_sz, if _flag == 0 {"compressed"} else {"uncompressed"});
                    i += 1;
                }
            },
            SQFS_INODE_TYPE_SQFS_INODE_DIR => {
                println!("Start block: {}", (*inode).data.dir.start_block);
                println!("Offset: {}", (*inode).data.dir.offset);
                println!("Listing size: {}", (*inode).data.dir.size);
                println!("Parent inode: {}", (*inode).data.dir.parent_inode);
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_DIR => {
                println!("Start block: {}", (*inode).data.dir_ext.start_block);
                println!("Offset: {}", (*inode).data.dir_ext.offset);
                println!("Listing size: {}", (*inode).data.dir_ext.size);
                println!("Parent inode: {}", (*inode).data.dir_ext.parent_inode);
                println!("Directory index entries: {}", (*inode).data.dir_ext.inodex_count);

                if (*inode).data.dir_ext.size == 0 {
                    return;
                }

                let mut idx = MaybeUninit::<*mut sqfs_dir_index_t>::uninit();
                let mut i = 0;
                loop {
                    let ret = sqfs_inode_unpack_dir_index_entry(inode, idx.as_mut_ptr(), i);
                    if ret == SQFS_ERROR_SQFS_ERROR_OUT_OF_BOUNDS {
                        break;
                    }
                    if ret < 0 {
                        println!("ERROR reading directory index");
                        return;
                    }
                    let idxp = idx.assume_init();
                    println!("\t{} -> block {}, header offset {}",
                        CStr::from_ptr((*idxp).name.as_ptr() as *const std::ffi::c_char).to_str().unwrap(),
                        (*idxp).start_block, (*idxp).index);

                    sqfs_free(idxp as *mut c_void);
                    i += 1;
                }
            },
            _ => {
                unimplemented!("file type not yet implemented");
            },
        }

        } // end of unsafe
    } // end of fn
}
