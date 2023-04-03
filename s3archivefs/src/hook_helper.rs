use libc::{c_void, c_int, off_t, lseek};
use log::{debug, error};
use crate::bindings::*;
use crate::repo::{CONTEXT, HoleDetectMode};

pub type WriteAtType = unsafe extern "C" fn(*mut sqfs_file_t, sqfs_u64, *const c_void, usize) -> c_int;
pub type ReadAtType = unsafe extern "C" fn(*mut sqfs_file_t, sqfs_u64, *mut c_void, usize) -> c_int;

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

impl sqfs_file_stdio_t {
    pub fn get_size(&self) -> usize {
        self.size as usize
    }
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
                let errno: c_int =  std::io::Error::last_os_error().raw_os_error().unwrap().into();
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
