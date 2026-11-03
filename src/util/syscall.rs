use std::io;
use anyhow::{ Context, Result };

/* A syscall's -1 as an error naming the call, e.g. "KVM_SET_REGS: Invalid
 * argument (os error 22)". errno is read here, so pass the call's return
 * value straight in, with nothing that may change errno in between. */
pub fn syscall_check(name: &'static str, ret: libc::c_int) -> Result<libc::c_int> {
    if ret < 0 {
        return Err(io::Error::last_os_error()).context(name);
    }
    Ok(ret)
}

/* The same for mmap(), which fails with MAP_FAILED. */
pub fn mmap_check(name: &'static str, ptr: *mut libc::c_void) -> Result<*mut libc::c_void> {
    if ptr == libc::MAP_FAILED {
        return Err(io::Error::last_os_error()).context(name);
    }
    Ok(ptr)
}
