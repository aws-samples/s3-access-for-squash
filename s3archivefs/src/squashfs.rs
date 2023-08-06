use std::ptr;
use std::io::Error;
use std::ffi::{CString, CStr};
use log::{info, debug, warn};
use libc;
use libc::{c_char, c_void, c_int, size_t};
use crate::bindings::*;
use crate::hook_helper::*;
use super::*;

#[allow(non_camel_case_types)]
pub type sqfs_readdir_callback_t = Option<
    unsafe extern "C" fn(
        buf: *mut c_void,
        name: *const c_char,
        stbuf: *const libc::stat,
        filler: Option<unsafe extern "C" fn()>,
    ) -> c_int,
>;

pub struct DirReader<'a> {
    ctx: &'a mut Archive,
    dr: *mut sqfs_dir_reader_t,
}

impl<'a> DirReader<'a> {

    pub fn new(ctx: &'a mut Archive, dr: *mut sqfs_dir_reader_t) -> Self {

        Self {
            ctx: ctx,
            dr: dr,
        }
    }
}

impl<'a> Drop for DirReader<'a> {
    fn drop(&mut self) {
        sqfs_destroy(self.dr);
        debug!("struct DirReader dropped");
    }
}

impl<'a> Iterator for DirReader<'a> {
    type Item = (String, libc::stat);

    fn next(&mut self) -> Option<Self::Item> {

        unsafe {

        let mut ent: *mut sqfs_dir_entry_t = ptr::null_mut();
        let err = sqfs_dir_reader_read(self.dr, ptr::addr_of_mut!(ent));
        if err > 0 {
            debug!("sqfs_dir_reader_read no next");
            return None;
        }
        if err < 0 {
            debug!("sqfs_dir_reader_read failed, err: {}", err);
            return None;
        }

        let name = std::str::from_utf8_unchecked(
            std::slice::from_raw_parts((*ent).name.as_ptr(), (*ent).size as usize + 1)
        ).to_string();

        let mut inode: *mut sqfs_inode_generic_t = ptr::null_mut();
        let err = sqfs_dir_reader_get_inode(self.dr, ptr::addr_of_mut!(inode));
        if err != 0 {
            sqfs_free(ent as *mut c_void);
            debug!("failed to get inode for {:?}, err: {}", name, err);
            return None;
        }

        let st = self.ctx.generic_inode_to_stat(inode);

        sqfs_free(ent as *mut c_void);
        sqfs_free(inode as *mut c_void);

        Some((name, st))

        }
    }
}

#[repr(C)]
pub struct Archive {
    pub sb: sqfs_super_t,
    pub cfg: sqfs_compressor_config_t,
    pub cmp: *mut sqfs_compressor_t,
    pub file: *mut sqfs_file_t,
    pub idtbl: *mut sqfs_id_table_t,
}

impl Drop for Archive {
    fn drop(&mut self) {
        sqfs_destroy(self.idtbl);
        sqfs_destroy(self.cmp);
        sqfs_destroy(self.file);
        debug!("struct Archive dropped");
    }
}

impl ArchiveFs for Archive {

    fn get_sb(&self) -> sqfs_super_t {
        self.sb.clone()
    }

    fn get_archive_file_size(&self) -> usize {

        let file = self.file as *mut sqfs_file_stdio_t;
        unsafe {
            (*file).get_size()
        }
    }

    fn set_hook(&self) {
        info!("s3 archive fs hooked");
        unsafe {
            // hook read_at
            let read_at = (*self.file).read_at.replace(archive_read_at);
            let _ = (*self.file).write_at.replace(
                std::mem::transmute::<ReadAtType, WriteAtType>(read_at.unwrap())
            );
        }
    }

    fn extract_one(&self, path: &str, outpath: &str) -> Result<usize, Error> {
        let _ = path;
        let _ = outpath;
        unimplemented!();
    }

    fn print_list(&self, path: Option<String>) {
        let _ = path;
        unimplemented!();
    }

    fn print_file_stat(&self, filepath: &str) {
        let _ = filepath;
        unimplemented!();
    }

    fn file_list(&self, path: Option<String>) -> Vec<(String, libc::stat64)> {
        let _ = path;
        unimplemented!();
    }

    fn file_stat(&self, filepath: &str) -> Option<libc::stat64> {
        let _ = filepath;
        unimplemented!();
    }
}

impl Archive {

    pub fn new(path: &str) -> Box<impl ArchiveFs> {
        Box::new(Self::new_from_sparse(path, false))
    }

    pub fn new_from_sparse(path: &str, init_root: bool) -> impl ArchiveFs {
        let _ = init_root;
        unsafe {
            Self::new_from_file(path)
        }
    }

    unsafe fn new_from_file(filename: &str) -> Self {

        let mut ctx = Self {
            sb: std::mem::zeroed(),
            cfg: std::mem::zeroed(),
            cmp: ptr::null_mut(),
            file: ptr::null_mut(),
            idtbl: ptr::null_mut(),
        };

        // ownership transfer to ptr
        let filename_ptr = CString::new(filename).unwrap().into_raw();
        let file = sqfs_open_file(filename_ptr, SQFS_FILE_OPEN_FLAGS_SQFS_FILE_OPEN_READ_ONLY);
        // retake ptr to free memory
        let _ = CString::from_raw(filename_ptr);
        if file.is_null() {
            panic!("can not open file {}", filename);
        }

        let ret = sqfs_super_read(ptr::addr_of_mut!(ctx.sb), file);
        if ret > 0 {
            panic!("error reading super block");
        }

        ctx.file = file;

        sqfs_compressor_config_init(ptr::addr_of_mut!(ctx.cfg),
                        ctx.sb.compression_id as u32,
                        ctx.sb.block_size as usize,
                        SQFS_COMP_FLAG_SQFS_COMP_FLAG_UNCOMPRESS as u16);

        let ret = sqfs_compressor_create(ptr::addr_of_mut!(ctx.cfg), ptr::addr_of_mut!(ctx.cmp));
        if ret != 0 {
            panic!("error creating compressor");
        }

        let idtbl = sqfs_id_table_create(0);
        if idtbl.is_null() {
            panic!("error creating ID table");
        }

        let ret = sqfs_id_table_read(idtbl, file, ptr::addr_of_mut!(ctx.sb), ctx.cmp);
        if ret != 0 {
            panic!("error loading ID table");
        }
        ctx.idtbl = idtbl;

        ctx
    }

    pub unsafe fn read(&mut self, path: *const c_char, buf: *mut c_char, size: size_t, offset: off_t) -> c_int {

        debug!("read - path: {}, size: {}, offset: {}",
            CStr::from_ptr(path).to_str().unwrap(), size, offset);

        let mut root: *mut sqfs_inode_generic_t = ptr::null_mut();
        let mut inode: *mut sqfs_inode_generic_t = ptr::null_mut();

        let dr = sqfs_dir_reader_create(ptr::addr_of_mut!(self.sb), self.cmp, self.file, 0x1);
        if dr.is_null() {
            panic!("can not create dir reader");
        }
        sqfs_dir_reader_get_root_inode(dr, ptr::addr_of_mut!(root));

        let ret = sqfs_dir_reader_find_by_path(dr, root, path, ptr::addr_of_mut!(inode));
        sqfs_free(root as *mut c_void);
        sqfs_destroy(dr as *mut c_void);
        if ret != 0 {
            return -libc::ENOENT;
        }

        let data = sqfs_data_reader_create(self.file, self.sb.block_size as usize, self.cmp, 0);
        if data.is_null() {
            panic!("can not create data reader");
        }

        let ret = sqfs_data_reader_load_fragment_table(data, ptr::addr_of_mut!(self.sb));
        if ret != 0 {
            panic!("can not load fragment table");
        }

        let mut remain = size as u32;
        let buf_ptr = buf as *mut c_void;
        let mut off: usize = 0;
        loop {
            let diff = sqfs_data_reader_read(data, inode, offset as u64 + off as u64, buf_ptr.add(off), remain);
            if diff == 0 {
                break;
            }
            if diff < 0 {
                sqfs_free(inode as *mut c_void);
                sqfs_destroy(data as *mut c_void);
                return -libc::EIO;
            }
            off += diff as usize;
            remain -= diff as u32;
            if remain == 0 {
                break;
            }
        };
        sqfs_free(inode as *mut c_void);
        sqfs_destroy(data as *mut c_void);
        off as i32
    }

    pub unsafe fn readdir<'a>(&'a mut self, path: *const c_char) -> Option<DirReader> {

        debug!("readdir - path: {}", CStr::from_ptr(path).to_str().unwrap());

        let mut root: *mut sqfs_inode_generic_t = ptr::null_mut();
        let mut inode: *mut sqfs_inode_generic_t = ptr::null_mut();

        let dr = sqfs_dir_reader_create(ptr::addr_of_mut!(self.sb), self.cmp, self.file, 0x1);
        if dr.is_null() {
            panic!("can not create dir reader");
        }
        sqfs_dir_reader_get_root_inode(dr, ptr::addr_of_mut!(root));

        let ret = sqfs_dir_reader_find_by_path(dr, root, path, ptr::addr_of_mut!(inode));
        sqfs_free(root as *mut c_void);
        if ret != 0 {
            debug!("not able to find inode for path: {}", CStr::from_ptr(path).to_str().unwrap());
            return None;
        }

        let ret = sqfs_dir_reader_open_dir(dr, inode, 0);
        sqfs_free(inode as *mut c_void);
        if ret != 0 {
            warn!("failed to open dir for inode");
            return None;
        }

        Some(DirReader::new(self, dr))
    }

    pub unsafe fn readdir_cb(&mut self, path: *const c_char, buf: *mut c_void,
            filler: Option<unsafe extern "C" fn()>, cb: sqfs_readdir_callback_t) -> c_int {

        let cb_func = cb.unwrap();
        debug!("readdir - path: {}", CStr::from_ptr(path).to_str().unwrap());

        let mut root: *mut sqfs_inode_generic_t = ptr::null_mut();
        let mut inode: *mut sqfs_inode_generic_t = ptr::null_mut();

        let dr = sqfs_dir_reader_create(ptr::addr_of_mut!(self.sb), self.cmp, self.file, 0x1);
        if dr.is_null() {
            panic!("can not create dir reader");
        }
        sqfs_dir_reader_get_root_inode(dr, ptr::addr_of_mut!(root));

        let ret = sqfs_dir_reader_find_by_path(dr, root, path, ptr::addr_of_mut!(inode));
        sqfs_free(root as *mut c_void);
        if ret != 0 {
            debug!("not able to find inode for path: {}", CStr::from_ptr(path).to_str().unwrap());
            return -libc::ENOENT;
        }

        let ret = sqfs_dir_reader_open_dir(dr, inode, 0);
        sqfs_free(inode as *mut c_void);
        if ret != 0 {
            warn!("failed to open dir for inode");
            return -libc::ENOENT;
        }

        loop {
            let mut ent: *mut sqfs_dir_entry_t = ptr::null_mut();
            let err = sqfs_dir_reader_read(dr, ptr::addr_of_mut!(ent));
            if err > 0 {
                break;
            }
            if err < 0 {
                sqfs_destroy(dr as *mut c_void);
                return err;
            }
            // should_skip
            //

            let mut inode: *mut sqfs_inode_generic_t = ptr::null_mut();
            let err = sqfs_dir_reader_get_inode(dr, ptr::addr_of_mut!(inode));
            if err > 0 {
                sqfs_free(ent as *mut c_void);
                sqfs_destroy(dr as *mut c_void);
                return err;
            }

            let st = self.generic_inode_to_stat(inode);
            cb_func(buf, (*ent).name.as_ptr() as *const c_char, ptr::addr_of!(st), filler);

            sqfs_free(ent as *mut c_void);
        }
        sqfs_destroy(dr as *mut c_void);

        0
    }

    pub unsafe fn readlink(&mut self, path: *const c_char, buf: *mut c_char, size: size_t) -> c_int {

        debug!("readlink() - path: {}, size: {}", CStr::from_ptr(path).to_str().unwrap(), size);

        let mut root: *mut sqfs_inode_generic_t = ptr::null_mut();
        let mut inode: *mut sqfs_inode_generic_t = ptr::null_mut();

        let dr = sqfs_dir_reader_create(ptr::addr_of_mut!(self.sb), self.cmp, self.file, 0x1);
        if dr.is_null() {
            panic!("can not create dir reader");
        }
        sqfs_dir_reader_get_root_inode(dr, ptr::addr_of_mut!(root));

        let ret = sqfs_dir_reader_find_by_path(dr, root, path, ptr::addr_of_mut!(inode));
        sqfs_free(root as *mut c_void);
        sqfs_destroy(dr as *mut c_void);
        if ret != 0 {
            return -libc::ENOENT;
        }

        let ret;
        match (*inode).base.type_ as u32 {
            SQFS_INODE_TYPE_SQFS_INODE_SLINK => {
                let link_size = (*inode).data.slink.target_size as usize;
                if link_size + 1 > size {
                    ret = -libc::EIO;
                } else {
                    let link_target = (*inode).extra.as_ptr() as *const c_void;
                    libc::memset(buf as *mut c_void, 0, link_size + 1);
                    libc::memcpy(buf as *mut c_void, link_target, link_size);
                    ret = 0;
                }
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_SLINK => {
                let link_size = (*inode).data.slink_ext.target_size as usize;
                if link_size + 1 > size {
                    ret = - libc::EIO;
                } else {
                    let link_target = (*inode).extra.as_ptr() as *const c_void;
                    libc::memset(buf as *mut c_void, 0, link_size + 1);
                    libc::memcpy(buf as *mut c_void, link_target, link_size);
                    ret = 0;
                }
            },
            _ => {
                ret = 0;
            },
        }
        sqfs_free(inode as *mut c_void);
        ret
    }

    pub unsafe fn getattr(&mut self, path: *const c_char, stbuf: *mut libc::stat) -> c_int {

        debug!("getattr - path: {}", CStr::from_ptr(path).to_str().unwrap());

        let mut root: *mut sqfs_inode_generic_t = ptr::null_mut();
        let mut inode: *mut sqfs_inode_generic_t = ptr::null_mut();

        let dr = sqfs_dir_reader_create(ptr::addr_of_mut!(self.sb), self.cmp, self.file, 0x1);
        if dr.is_null() {
            panic!("can not create dir reader");
        }
        sqfs_dir_reader_get_root_inode(dr, ptr::addr_of_mut!(root));

        let ret = sqfs_dir_reader_find_by_path(dr, root, path, ptr::addr_of_mut!(inode));
        sqfs_free(root as *mut c_void);
        sqfs_destroy(dr as *mut c_void);
        if ret != 0 {
            return -libc::ENOENT;
        }

        (*stbuf) = self.generic_inode_to_stat(inode);
        sqfs_free(inode as *mut c_void);

        0
    }

    pub unsafe fn getxattr(&mut self, path: *const c_char, name: *const c_char, value: *mut c_char, size: size_t) -> c_int {

        debug!("getxattr() - path: {}, name: {}, size: {}",
            CStr::from_ptr(path).to_str().unwrap(), CStr::from_ptr(name).to_str().unwrap(), size);

        if name.is_null() {
            return -libc::ENODATA;
        }

        let name_len = libc::strlen(name);
        if name_len == 0 {
            return -libc::ENODATA;
        }

        let mut root: *mut sqfs_inode_generic_t = ptr::null_mut();
        let mut inode: *mut sqfs_inode_generic_t = ptr::null_mut();

        let dr = sqfs_dir_reader_create(ptr::addr_of_mut!(self.sb), self.cmp, self.file, 0x1);
        if dr.is_null() {
            panic!("can not create dir reader");
        }
        sqfs_dir_reader_get_root_inode(dr, ptr::addr_of_mut!(root));

        let ret = sqfs_dir_reader_find_by_path(dr, root, path, ptr::addr_of_mut!(inode));
        sqfs_free(root as *mut c_void);
        sqfs_destroy(dr as *mut c_void);
        if ret != 0 {
            return -libc::ENODATA;
        }

        let mut xattr_idx = 0xFFFFFFFF;

        match (*inode).base.type_ as u32 {
            SQFS_INODE_TYPE_SQFS_INODE_EXT_BDEV |
            SQFS_INODE_TYPE_SQFS_INODE_EXT_CDEV => {
                xattr_idx = (*inode).data.dev_ext.xattr_idx;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_FIFO |
            SQFS_INODE_TYPE_SQFS_INODE_EXT_SOCKET => {
                xattr_idx = (*inode).data.ipc_ext.xattr_idx;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_SLINK => {
                xattr_idx = (*inode).data.slink_ext.xattr_idx;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_FILE => {
                xattr_idx = (*inode).data.file_ext.xattr_idx;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_DIR => {
                xattr_idx = (*inode).data.dir_ext.xattr_idx;
            },
            _ => {
                debug!("type {} is not a ext inode", (*inode).base.type_);
            }
        }

        if xattr_idx == 0xFFFFFFFF {
            sqfs_free(inode as *mut c_void);
            return -libc::ENODATA;
        }

        let mut desc: sqfs_xattr_id_t = std::mem::zeroed();

        let xr = sqfs_xattr_reader_create(0);
        if xr.is_null() {
            panic!("error creating xattr reader");
        }

        let ret = sqfs_xattr_reader_load(xr, std::ptr::addr_of!(self.sb), self.file, self.cmp);
        if ret != 0 {
            panic!("error loading xattr reader");
        }

        if sqfs_xattr_reader_get_desc(xr, xattr_idx, std::ptr::addr_of_mut!(desc)) != 0 {
            panic!("unable to resolve xattr idx: {}", xattr_idx);
        }

        let ret = sqfs_xattr_reader_seek_kv(xr, std::ptr::addr_of_mut!(desc));
        if ret != 0 {
            panic!("error locating xattr key-value pairs");
        }

        let mut key: *mut sqfs_xattr_entry_t = ptr::null_mut();
        let mut val: *mut sqfs_xattr_value_t = ptr::null_mut();

        let mut count = desc.count;
        let mut val_size: c_int = 0;
        while count > 0 {

            let ret = sqfs_xattr_reader_read_key(xr, std::ptr::addr_of_mut!(key));
            if ret != 0 {
                panic!("error reading xattr key, err: {}", ret);
            }

            let ret = sqfs_xattr_reader_read_value(xr, key, std::ptr::addr_of_mut!(val));
            if ret != 0 {
                sqfs_free(key as *mut c_void);
                panic!("error reading xattr value");
            }

            debug!("found xattr key {} - size {}, val {} - size {}",
                CStr::from_ptr((*key).key.as_ptr() as *const i8).to_str().unwrap(), (*key).size,
                CStr::from_ptr((*val).value.as_ptr() as *const i8).to_str().unwrap(), (*val).size);

            let ret = libc::strncmp(name, (*key).key.as_ptr() as *const i8, name_len);
            if ret == 0 {
                val_size = (*val).size as c_int;
                if size != 0 {
                    if val_size <= size as c_int {
                        libc::memcpy(value as *mut c_void, (*val).value.as_ptr() as *const c_void, val_size as usize);
                    } else {
                        val_size = -libc::ERANGE;
                    }
                } else {
                    // if input size is zero, do nothing
                    // just return size of value
                }
                sqfs_free(key as *mut c_void);
                sqfs_free(val as *mut c_void);
                break;
            }
            sqfs_free(key as *mut c_void);
            sqfs_free(val as *mut c_void);
            count -= 1;
        }

        sqfs_free(inode as *mut c_void);
        sqfs_destroy(xr as *mut c_void);

        val_size as c_int
    }

    pub unsafe fn listxattr(&mut self, path: *const c_char, list: *mut c_char, size: size_t) -> c_int {

        debug!("listxattr() - path: {}, size: {}", CStr::from_ptr(path).to_str().unwrap(), size);

        let mut root: *mut sqfs_inode_generic_t = ptr::null_mut();
        let mut inode: *mut sqfs_inode_generic_t = ptr::null_mut();

        let dr = sqfs_dir_reader_create(ptr::addr_of_mut!(self.sb), self.cmp, self.file, 0x1);
        if dr.is_null() {
            panic!("can not create dir reader");
        }
        sqfs_dir_reader_get_root_inode(dr, ptr::addr_of_mut!(root));

        let ret = sqfs_dir_reader_find_by_path(dr, root, path, ptr::addr_of_mut!(inode));
        sqfs_free(root as *mut c_void);
        sqfs_destroy(dr as *mut c_void);
        if ret != 0 {
            return -libc::ENOENT;
        }

        let mut xattr_idx = 0xFFFFFFFF;

        match (*inode).base.type_ as u32 {
            SQFS_INODE_TYPE_SQFS_INODE_EXT_BDEV |
            SQFS_INODE_TYPE_SQFS_INODE_EXT_CDEV => {
                xattr_idx = (*inode).data.dev_ext.xattr_idx;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_FIFO |
            SQFS_INODE_TYPE_SQFS_INODE_EXT_SOCKET => {
                xattr_idx = (*inode).data.ipc_ext.xattr_idx;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_SLINK => {
                xattr_idx = (*inode).data.slink_ext.xattr_idx;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_FILE => {
                xattr_idx = (*inode).data.file_ext.xattr_idx;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_DIR => {
                xattr_idx = (*inode).data.dir_ext.xattr_idx;
            },
            _ => {
                debug!("type {} is not a ext inode", (*inode).base.type_);
            }
        }

        if xattr_idx == 0xFFFFFFFF {
            sqfs_free(inode as *mut c_void);
            return -libc::ENOENT;
        }

        let mut desc: sqfs_xattr_id_t = std::mem::zeroed();

        let xr = sqfs_xattr_reader_create(0);
        if xr.is_null() {
            panic!("error creating xattr reader");
        }

        let ret = sqfs_xattr_reader_load(xr, std::ptr::addr_of!(self.sb), self.file, self.cmp);
        if ret != 0 {
            panic!("error loading xattr reader");
        }

        if sqfs_xattr_reader_get_desc(xr, xattr_idx, std::ptr::addr_of_mut!(desc)) != 0 {
            panic!("unable to resolve xattr idx: {}", xattr_idx);
        }

        let ret = sqfs_xattr_reader_seek_kv(xr, std::ptr::addr_of_mut!(desc));
        if ret != 0 {
            panic!("error locating xattr key-value pairs");
        }

        let mut key: *mut sqfs_xattr_entry_t = ptr::null_mut();
        let mut val: *mut sqfs_xattr_value_t = ptr::null_mut();

        let mut count = desc.count;
        let mut list_size: c_int = 0;
        libc::memset(list as *mut c_void, 0, size as usize);
        while count > 0 {

            let ret = sqfs_xattr_reader_read_key(xr, std::ptr::addr_of_mut!(key));
            if ret != 0 {
                panic!("error reading xattr key");
            }

            let ret = sqfs_xattr_reader_read_value(xr, key, std::ptr::addr_of_mut!(val));
            if ret != 0 {
                sqfs_free(key as *mut c_void);
                panic!("error reading xattr value");
            }

            let prefix = sqfs_get_xattr_prefix((*key).type_ as SQFS_XATTR_TYPE & SQFS_XATTR_TYPE_SQFS_XATTR_PREFIX_MASK);
            let prefix_len = libc::strlen(prefix) as c_int;

            if size != 0 {

                libc::memcpy((list as *mut c_void).offset(list_size as isize),
                    (*key).key.as_ptr() as *const c_void,
                    (*key).size as usize + prefix_len as usize);
            }

            list_size += (*key).size as c_int + prefix_len + 1;
            sqfs_free(key as *mut c_void);
            sqfs_free(val as *mut c_void);
            count -= 1;
        }

        sqfs_free(inode as *mut c_void);
        sqfs_destroy(xr as *mut c_void);

        list_size as c_int
    }

    pub unsafe fn generic_inode_to_stat(&mut self, inode: *mut sqfs_inode_generic_t) -> libc::stat {

        let mut st: libc::stat = std::mem::zeroed();
        let mut xattr_idx = 0xFFFFFFFF;

        // follow kernel behave fs/squashfs/inode.c
        match (*inode).base.type_ as u32 {
            SQFS_INODE_TYPE_SQFS_INODE_BDEV |
            SQFS_INODE_TYPE_SQFS_INODE_CDEV => {
                st.st_nlink = (*inode).data.dev.nlink as u64;
                st.st_rdev = (*inode).data.dev.devno as u64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_BDEV |
            SQFS_INODE_TYPE_SQFS_INODE_EXT_CDEV => {
                st.st_nlink = (*inode).data.dev_ext.nlink as u64;
                st.st_rdev = (*inode).data.dev_ext.devno as u64;
                xattr_idx = (*inode).data.dev_ext.xattr_idx;
            },
            SQFS_INODE_TYPE_SQFS_INODE_FIFO |
            SQFS_INODE_TYPE_SQFS_INODE_SOCKET => {
                st.st_nlink = (*inode).data.ipc.nlink as u64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_FIFO |
            SQFS_INODE_TYPE_SQFS_INODE_EXT_SOCKET => {
                st.st_nlink = (*inode).data.ipc_ext.nlink as u64;
                xattr_idx = (*inode).data.ipc_ext.xattr_idx;
            },
            SQFS_INODE_TYPE_SQFS_INODE_SLINK => {
                st.st_nlink = (*inode).data.slink.nlink as u64;
                st.st_size = (*inode).data.slink.target_size as i64;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_SLINK => {
                st.st_nlink = (*inode).data.slink_ext.nlink as u64;
                st.st_size = (*inode).data.slink_ext.target_size as i64;
                xattr_idx = (*inode).data.slink_ext.xattr_idx;
            },
            SQFS_INODE_TYPE_SQFS_INODE_FILE => {
                st.st_nlink = 0;
                st.st_size = (*inode).data.file.file_size as i64;
                st.st_blksize = 4096;
                st.st_blocks = ((st.st_size - 1) >> 9) + 1;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_FILE => {
                st.st_nlink = (*inode).data.file_ext.nlink as u64;
                st.st_size = (*inode).data.file_ext.file_size as i64;
                st.st_blksize = 4096;
                st.st_blocks = (st.st_size - (*inode).data.file_ext.sparse as i64 + 511) >> 9;
                xattr_idx = (*inode).data.file_ext.xattr_idx;
            },
            SQFS_INODE_TYPE_SQFS_INODE_DIR => {
                st.st_nlink = (*inode).data.dir.nlink as u64;
                st.st_size = (*inode).data.dir.size as i64;
                st.st_blksize = 4096;
                st.st_blocks = 0;
            },
            SQFS_INODE_TYPE_SQFS_INODE_EXT_DIR => {
                st.st_nlink = (*inode).data.dir_ext.nlink as u64;
                st.st_size = (*inode).data.dir_ext.size as i64;
                st.st_blksize = 4096;
                st.st_blocks = 0;
                xattr_idx = (*inode).data.dir_ext.xattr_idx;
            },
             _ => {
                 todo!();
             }
        }

        st.st_ino = (*inode).base.inode_number as u64;
        st.st_mode = (*inode).base.mode as u32;
        st.st_ctime = (*inode).base.mod_time as i64;
        st.st_atime = (*inode).base.mod_time as i64;
        st.st_mtime = (*inode).base.mod_time as i64;

        let mut uid = 9999999;
        let ret = sqfs_id_table_index_to_id(self.idtbl, (*inode).base.uid_idx, ptr::addr_of_mut!(uid));
        if ret == 0 {
            st.st_uid = uid;
        }

        let mut gid = 9999999;
        let ret = sqfs_id_table_index_to_id(self.idtbl, (*inode).base.gid_idx, ptr::addr_of_mut!(gid));
        if ret == 0 {
            st.st_gid = gid;
        }

        if xattr_idx != 0xFFFFFFFF {

            let mut desc: sqfs_xattr_id_t = std::mem::zeroed();

            let xattr_rd = sqfs_xattr_reader_create(0);
            if xattr_rd.is_null() {
                panic!("error creating xattr reader");
            }

            let ret = sqfs_xattr_reader_load(xattr_rd, ptr::addr_of_mut!(self.sb), self.file, self.cmp);
            if ret != 0 {
                panic!("error loading xattr reader");
            }

            if sqfs_xattr_reader_get_desc(xattr_rd, xattr_idx, ptr::addr_of_mut!(desc)) != 0 {
                panic!("unable to resolve xattr idx: {}", xattr_idx);
            }

            if desc.size > 0 {
                st.st_blocks += ((desc.size as i64 - 1) >> 9) + 1;
            }
        }

        st
    }
}
