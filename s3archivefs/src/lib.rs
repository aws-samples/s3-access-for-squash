pub mod squashfs_v1;
pub mod squashfs;
pub mod repo;
pub mod transfer;
pub mod stats;
pub mod hook_helper;

pub mod bindings {
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(non_upper_case_globals)]
    #![allow(dead_code)]
    include!("bindings.rs");
}

use bindings::*;

fn sqfs_destroy<T>(x: *mut T) {
    unsafe {
        if x.is_null() {
            return;
        }
        let obj = x as *mut sqfs_object_t;
        ((*obj).destroy.unwrap())(obj);
    }
}

pub trait ArchiveFs {
    fn new(path: &str) -> Box<Self>;
    fn get_sb(&self) -> sqfs_super_t;
    fn get_archive_file_size(&self) -> usize;
    fn set_hook(&self);
    fn extract_one(&self, path: &str, outpath: &str) -> Result<usize, std::io::Error>;
    fn print_list(&self, path: Option<String>);
    fn print_file_stat(&self, filepath: &str);
    fn file_list(&self, path: Option<String>) -> Vec<(String, libc::stat64)>;
    fn file_stat(&self, filepath: &str) -> Option<libc::stat64>;
}
