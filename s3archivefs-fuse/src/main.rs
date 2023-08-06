use std::env;
use std::rc::Rc;
use std::ffi::{CString, CStr};
use std::collections::VecDeque;
use aws_config::meta::region::RegionProviderChain;
use libc::{c_int, c_char, c_void, off_t, size_t};
use libfuse_sys::fuse;
use log::{info, debug};
use env_logger;
use tokio;
use s3archivefs::squashfs::Archive;
use s3archivefs::repo::{Remote, Local, HoleDetectMode, CONTEXT};

unsafe extern "C" fn ops_init(conn: *mut fuse::fuse_conn_info, config: *mut fuse::fuse_config) -> *mut c_void
{
    debug!("ops_init -");
    (*config).kernel_cache = 1;
    (*config).use_ino = 1;
    if ((*conn).capable & fuse::FUSE_CAP_READDIRPLUS) > 0 {
        debug!("FUSE_CAP_READDIRPLUS is set");
    }

    let fuse_ctx = fuse::fuse_get_context();
    (*fuse_ctx).private_data
}

unsafe extern "C" fn ops_destroy(private_data: *mut c_void)
{
    debug!("ops_destroy -");

    let _ = private_data;
}

unsafe extern "C" fn ops_open(path: *const c_char, fi: *mut fuse::fuse_file_info) -> c_int
{
    debug!("ops_open - path: {}", CStr::from_ptr(path).to_str().unwrap());

    let _ = fi;

    if ((*fi).flags & libc::O_ACCMODE) != libc::O_RDONLY {
        return -libc::EACCES;
    }
    0
}

unsafe extern "C" fn ops_getattr(path: *const c_char, stbuf: *mut libc::stat, fi: *mut fuse::fuse_file_info) -> c_int
{
    debug!("ops_getattr - path: {}", CStr::from_ptr(path).to_str().unwrap());

    let _ = fi;
    let fuse_ctx = fuse::fuse_get_context();

    let mut rc = Rc::from_raw((*fuse_ctx).private_data as *mut Archive);
    debug!("ops_getattr - ref count {}", Rc::strong_count(&rc));
    let arcfs = Rc::get_mut(&mut rc).unwrap();

    let ret = arcfs.getattr(path, stbuf);

    let _ = Rc::into_raw(rc);
    ret
}

#[allow(dead_code)]
unsafe extern "C" fn fuse_readdir_cb(buf: *mut c_void, name: *const c_char,
        stbuf: *const libc::stat, filler: Option<unsafe extern "C" fn()>) -> c_int {

    let filler_func = std::mem::transmute::<Option<unsafe extern "C" fn()>, fuse::fuse_fill_dir_t>(filler).unwrap();
    filler_func(buf, name, stbuf, 0, fuse::fuse_fill_dir_flags_FUSE_FILL_DIR_PLUS);
    0
}

unsafe extern "C" fn ops_readdir(path: *const c_char, buf: *mut c_void, filler: fuse::fuse_fill_dir_t,
        offset: off_t, fi: *mut fuse::fuse_file_info, flags: fuse::fuse_readdir_flags) -> c_int
{
    let filler_func = filler.unwrap();
    debug!("ops_readdir - path: {}, flag: {}", CStr::from_ptr(path).to_str().unwrap(), flags);

    let _ = offset;
    let _ = fi;

    let fuse_ctx = fuse::fuse_get_context();
    let mut rc = Rc::from_raw((*fuse_ctx).private_data as *mut Archive);
    let arcfs = Rc::get_mut(&mut rc).unwrap();

    /*
    // method 1
    arcfs.readdir_cb(path, buf,
        std::mem::transmute::<fuse::fuse_fill_dir_t, Option<unsafe extern "C" fn()>>(filler),
        Some(fuse_readdir_cb)
    );
    */
    // method 2
    match arcfs.readdir(path) {
        None => {},
        Some(dr) => {
            let _ = dr.map(|(name, st)| {
                // ownership transferr to ptr
                let name_ptr = CString::new(name).expect("failed to cstring").into_raw();

                filler_func(buf, name_ptr, std::ptr::addr_of!(st), 0, fuse::fuse_fill_dir_flags_FUSE_FILL_DIR_PLUS);

                // retake ptr to free memory
                let _ = CString::from_raw(name_ptr);
            }).collect::<Vec<_>>();
        }
    }
    let _ = Rc::into_raw(rc);
    0
}

unsafe extern "C" fn ops_read(path: *const c_char, buf: *mut c_char, size: size_t,
        offset: off_t, fi: *mut fuse::fuse_file_info) -> c_int
{
    debug!("ops_read - path: {}, size: {}, offset: {}",
        CStr::from_ptr(path).to_str().unwrap(), size, offset);

    let _ = fi;

    let fuse_ctx = fuse::fuse_get_context();
    let mut rc = Rc::from_raw((*fuse_ctx).private_data as *mut Archive);
    let arcfs = Rc::get_mut(&mut rc).unwrap();

    let ret = arcfs.read(path, buf, size, offset);

    let _ = Rc::into_raw(rc);
    ret
}

unsafe extern "C" fn ops_readlink(path: *const c_char, buf: *mut c_char, size: size_t) -> c_int
{
    debug!("ops_readlink - path: {}, size: {}", CStr::from_ptr(path).to_str().unwrap(), size);

    let fuse_ctx = fuse::fuse_get_context();
    let mut rc = Rc::from_raw((*fuse_ctx).private_data as *mut Archive);
    let arcfs = Rc::get_mut(&mut rc).unwrap();

    let ret = arcfs.readlink(path, buf, size);

    let _ = Rc::into_raw(rc);
    ret
}

unsafe extern "C" fn ops_release(path: *const c_char, fi: *mut fuse::fuse_file_info) -> c_int
{
    debug!("ops_release - path: {}", CStr::from_ptr(path).to_str().unwrap());

    let _ = fi;

    return 0;
}

unsafe extern "C" fn ops_getxattr(path: *const c_char, name: *const c_char, value: *mut c_char, size: size_t) -> c_int
{
    debug!("ops_getxattr - path: {}, name: {}, size: {}",
        CStr::from_ptr(path).to_str().unwrap(), CStr::from_ptr(name).to_str().unwrap(), size);

    if name.is_null() {
        return 0;
    }

    let key_len = libc::strlen(name);
    if key_len == 0 {
        return 0;
    }

    let fuse_ctx = fuse::fuse_get_context();
    let mut rc = Rc::from_raw((*fuse_ctx).private_data as *mut Archive);
    let arcfs = Rc::get_mut(&mut rc).unwrap();

    let ret = arcfs.getxattr(path, name, value, size);

    let _ = Rc::into_raw(rc);
    ret
}

unsafe extern "C" fn ops_listxattr(path: *const c_char, list: *mut c_char, size: size_t) -> c_int
{
    debug!("ops_listxattr - path: {}, size: {}", CStr::from_ptr(path).to_str().unwrap(), size);

    let fuse_ctx = fuse::fuse_get_context();
    let mut rc = Rc::from_raw((*fuse_ctx).private_data as *mut Archive);
    let arcfs = Rc::get_mut(&mut rc).unwrap();

    let ret = arcfs.listxattr(path, list, size);

    let _ = Rc::into_raw(rc);
    ret
} 

fn show_help(args: VecDeque<String>) {

    let fuse_ops: fuse::fuse_operations = fuse::fuse_operations {
        ..Default::default()
    };

    let mut cs_args: Vec<*mut c_char> =
        args.iter().map(|x| CString::new(x.clone()).unwrap().into_raw()).collect();

    let c_argc: c_int = cs_args.len().try_into().unwrap();
    let c_argv: *mut *mut c_char = cs_args.as_mut_ptr();

    let exec = std::env::current_exe().unwrap();

    println!("USAGE:");
    println!("  {} [OPTIONS] [FUSE OPTIONS] mountpoint", exec.display());
    println!("OPTIONS:");
    println!("  -b, --bucket\t\tBucket of archive object in Amazon S3");
    println!("  -k, --key\t\tKey of archive object in Amazon S3");
    println!("  -c, --cache\t\tLocal cache file");
    println!("OPTIONAL:");
    println!("  -r, --region\t\tRegion of archive object in Amazon S3");
    println!("  -s, --size\t\tSize of chunk when read data from Amazon S3,");
    println!("\t\t\twhich NO less than underlayer block size. DEFAULT: block size");
    println!("  -h, --help\t\tThis help message");
    println!("\nShow FUSE help below:\n");

    unsafe {
        let _ = fuse::fuse_main(c_argc, c_argv, &fuse_ops as *const fuse::fuse_operations, std::ptr::null_mut());
    }
}

fn main() {

    env_logger::init();

    let fuse_ops: fuse::fuse_operations = fuse::fuse_operations {
        open: Some(ops_open),
        release: Some(ops_release),
        getattr: Some(ops_getattr),
        readlink: Some(ops_readlink),
        read: Some(ops_read),
        getxattr: Some(ops_getxattr),
        listxattr: Some(ops_listxattr),
        readdir: Some(ops_readdir),
        init: Some(ops_init),
        destroy: Some(ops_destroy),
        ..Default::default()
    };

    let mut args: VecDeque<String> = env::args().collect();
    let exec = args.pop_front().unwrap();

    let mut rest_args = VecDeque::new();
    rest_args.push_back(exec);

    let mut help = false;
    let mut region = None;
    let mut bucket = None;
    let mut key = None;
    let mut cachefile = None;
    let mut chunksize = None;

    // app args filter
    while let Some(arg) = args.pop_front() {

        match arg.as_str() {
            "-r" | "--region" => {
                if let Some(next) = args.front() {
                    if !next.starts_with("-") {
                        region = args.pop_front();
                        continue;
                    }
                }
                panic!("please specify -r|--region <region>");
            },
            "-b" | "--bucket" => {
                if let Some(next) = args.front() {
                    if !next.starts_with("-") {
                        bucket = args.pop_front();
                        continue;
                    }
                }
                panic!("please specify -b|--bucket <bucket>");
            },
            "-k" | "--key" => {
                if let Some(next) = args.front() {
                    if !next.starts_with("-") {
                        key = args.pop_front();
                        continue;
                    }
                }
                panic!("please specify -k|--key <key>");
            },
            "-c" | "--cache" => {
                if let Some(next) = args.front() {
                    if !next.starts_with("-") {
                        cachefile = args.pop_front();
                        continue;
                    }
                }
                panic!("please specify -c|--cache <cachefile>");
            },
            "-s" | "--size" => {
                if let Some(next) = args.front() {
                    if !next.starts_with("-") {
                        chunksize = args.pop_front();
                        continue;
                    }
                }
            },
            "-h" | "--help" => {
                help = true;
                rest_args.push_back(arg)
            },
            _ => {
                rest_args.push_back(arg)
            }
        }

    }

    if help {
        show_help(rest_args);
        return();
    }

    // check MUST args
    if bucket.is_none() {
        panic!("please specify -b|--bucket <bucket>");
    }
    if key.is_none() {
        panic!("please specify -k|--key <key>");
    }
    if cachefile.is_none() {
        panic!("please specify -c|--cache <cachefile>");
    }

    let chunksize = chunksize.and_then(|x| x.parse::<usize>().ok());
    let bucket = bucket.unwrap();
    let key = key.unwrap();
    let cachefile = cachefile.unwrap();
    let hdmode = HoleDetectMode::LSEEK;
    let force = false;
    let init_root = false;

    // search for single thread setting
    if rest_args.iter().find(|x| x.as_str() == "-s") == None {
        info!("force single thread setting");
        rest_args.push_back("-s".to_string());
    }

    let mut cs_args: Vec<*mut c_char> =
        rest_args.iter().map(|x| CString::new(x.clone()).unwrap().into_raw()).collect();

    info!("rest args pass to fuse {:?}", rest_args);
    let c_argc: c_int = cs_args.len().try_into().unwrap();
    let c_argv: *mut *mut c_char = cs_args.as_mut_ptr();

    let fuse_args = fuse::fuse_args {
        argc: c_argc,
        argv: c_argv,
        allocated: 0,
    };

    let arcfs = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let default_region = RegionProviderChain::default_provider().region().await;
            let region = region.or(default_region
                                .map(|r| r.as_ref().to_string())
                            )
                            .expect("no region config found in cli or profile");
            info!("creating Remote - region: {}, bucket: {}, key: {}", region, bucket, key);
            let remote = Remote::new(&region, &bucket, &key).await;
            info!("creating Local - cache: {}, chunksize: {:?}, hdmode: LSEEK, force: {}, init_root: {}, last_ver: true",
                cachefile, chunksize, force, init_root);
            let local = Local::new(&cachefile, chunksize, hdmode, force, init_root, Some(remote.clone()), true).await;
            let arcfs = local.get_arcfs();
            CONTEXT.with(|c| *c.borrow_mut() = Some(local));
            arcfs
        });

    info!("starting fuse");
    unsafe {
        let _ = fuse::fuse_main(fuse_args.argc, fuse_args.argv, &fuse_ops as *const fuse::fuse_operations, arcfs as *mut c_void);
    }
}
