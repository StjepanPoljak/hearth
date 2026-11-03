use anyhow::Result;
use crate::mmap_check;

pub fn mmap_mem_region(mem_size: usize) -> Result<*mut libc::c_void> {
    // 140900 mmap(NULL, 1075838976, 0 /* PROT_NONE */, 0x22 /* MAP_PRIVATE|MAP_ANONYMOUS */, -1, 0) = 0x7768b3e00000
    // 140900 mmap(0x7768b3e00000, 1073741824, 0x3 /* PROT_READ|PROT_WRITE */, 0x32 /* MAP_PRIVATE|MAP_FIXED|MAP_ANONYMOUS */, -1, 0) = 0x7768b3e00000
    mmap_check("mmap of guest memory", unsafe {
	libc::mmap(std::ptr::null_mut(),
		   mem_size as usize,
		   libc::PROT_READ|libc::PROT_WRITE,
		   libc::MAP_PRIVATE|libc::MAP_ANONYMOUS,
		   -1,
		   0)
    })
}
