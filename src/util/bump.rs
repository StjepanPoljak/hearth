use anyhow::{ anyhow, Result };

/* Hands out guest memory in gpa_start..gpa_end, front to back, for what
 * the VMM puts in the guest before it runs: boot info, command lines,
 * modules. Nothing is ever freed. host_start is where gpa_start is
 * mapped in the VMM. */
pub struct BumpAllocator {
    gpa_start: u64,
    gpa_end: u64,
    next: u64,
    host_start: *mut u8
}

impl BumpAllocator {
    pub fn new(gpa_start: u64, gpa_end: u64, host_start: *mut u8) -> Self {
        BumpAllocator { gpa_start, gpa_end, next: gpa_start, host_start }
    }

    pub fn gpa_start(&self) -> u64 {
        self.gpa_start
    }

    pub fn gpa_end(&self) -> u64 {
        self.gpa_end
    }

    /* The GPA the next push or reserve starts at. */
    pub fn next(&self) -> u64 {
        self.next
    }

    /* Copies bytes in at the next free GPA and returns that GPA. */
    pub fn push(&mut self, bytes: &[u8]) -> Result<u64> {
        let gpa = self.next;
        let ptr = self.reserve(bytes.len())?;
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len()) };
        Ok(gpa)
    }

    /* Takes the next size bytes and returns where they are mapped in the
     * VMM, for the caller to fill in; their GPA is next() before the
     * call. */
    pub fn reserve(&mut self, size: usize) -> Result<*mut u8> {
        let end = self.next.checked_add(size as u64)
            .filter(|&end| end <= self.gpa_end)
            .ok_or_else(|| anyhow!(
                "{:#x} bytes at {:#x} cross the allocator's end, {:#x}.", size, self.next, self.gpa_end))?;
        let ptr = unsafe { self.host_start.add((self.next - self.gpa_start) as usize) };
        self.next = end;
        Ok(ptr)
    }

    /* Moves next up to a multiple of align, a power of two. */
    pub fn align(&mut self, align: u64) -> Result<u64> {
        let aligned = self.next.checked_next_multiple_of(align)
            .filter(|&a| a <= self.gpa_end)
            .ok_or_else(|| anyhow!(
                "Aligning {:#x} to {:#x} crosses the allocator's end, {:#x}.", self.next, align, self.gpa_end))?;
        self.next = aligned;
        Ok(aligned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alloc(mem: &mut [u8]) -> BumpAllocator {
        BumpAllocator::new(0x1000, 0x1000 + mem.len() as u64, mem.as_mut_ptr())
    }

    #[test]
    fn push_copies_and_bumps() {
        let mut mem = [0u8; 16];
        let mut a = alloc(&mut mem);
        assert_eq!(a.push(b"abc").unwrap(), 0x1000);
        assert_eq!(a.push(b"de").unwrap(), 0x1003);
        assert_eq!(a.next(), 0x1005);
        assert_eq!(&mem[..5], b"abcde");
    }

    #[test]
    fn reserve_returns_the_spot() {
        let mut mem = [0u8; 16];
        let base = mem.as_mut_ptr();
        let mut a = alloc(&mut mem);
        a.push(b"x").unwrap();
        assert_eq!(a.reserve(4).unwrap(), unsafe { base.add(1) });
        assert_eq!(a.next(), 0x1005);
    }

    #[test]
    fn up_to_the_end_fits() {
        let mut mem = [0u8; 16];
        let mut a = alloc(&mut mem);
        a.reserve(15).unwrap();
        a.push(b"z").unwrap();
        assert_eq!(a.next(), 0x1010);
        a.reserve(0).unwrap();
    }

    #[test]
    fn crossing_the_end_fails_and_changes_nothing() {
        let mut mem = [0u8; 16];
        let mut a = alloc(&mut mem);
        a.push(b"0123456789").unwrap();
        assert!(a.push(b"0123456").is_err());
        assert!(a.reserve(7).is_err());
        assert!(a.reserve(usize::MAX).is_err());
        assert_eq!(a.next(), 0x100a);
        assert_eq!(mem[10..], [0u8; 6]);
    }

    #[test]
    fn align() {
        let mut mem = [0u8; 16];
        let mut a = alloc(&mut mem);
        a.push(b"a").unwrap();
        assert_eq!(a.align(8).unwrap(), 0x1008);
        assert_eq!(a.align(8).unwrap(), 0x1008);
        assert!(a.align(0x20).is_err());
        assert_eq!(a.next(), 0x1008);
    }
}
