pub mod squashfs_v1;
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
