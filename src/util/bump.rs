use anyhow::{ anyhow, Result };
use zerocopy::{ Immutable, IntoBytes };

/* Hands out guest memory in gpa_start..gpa_end, front to back, for what
 * the VMM puts in the guest before it runs: boot info, command lines,
 * modules. Nothing is ever freed. host_start is where gpa_start is
 * mapped in the VMM. */
pub struct BumpAllocator {
    gpa_start: u64,
    gpa_end: u64,
    curr: u64,
    host_start: *mut u8,
}

impl BumpAllocator {
    pub fn new(gpa_start: u64, gpa_end: u64, host_start: *mut u8) -> Self {
        BumpAllocator { gpa_start, gpa_end, curr: gpa_start, host_start }
    }

    pub fn gpa_start(&self) -> u64 {
        self.gpa_start
    }

    pub fn gpa_end(&self) -> u64 {
        self.gpa_end
    }

    pub fn push<T: IntoBytes + Immutable>(&mut self, data: &[T]) -> Result<u64> {
        self.push_aligned(data, None)
    }

    /* Copies data's bytes in at the next free GPA that is a multiple of
     * both align and T's alignment, and returns that GPA. Byte order is
     * T's: give fields zerocopy's U32<LE> and the like to fix it. */
    pub fn push_aligned<T: IntoBytes + Immutable>(&mut self, data: &[T], align: Option<u64>) -> Result<u64> {
        let align = align.unwrap_or(1).max(std::mem::align_of::<T>() as u64);
        let curr = self.align(align)?;
        let bytes = data.as_bytes();
        let size = bytes.len() as u64;
        let curr_end = curr.checked_add(size)
            .filter(|&curr_end| curr_end <= self.gpa_end)
            .ok_or_else(|| anyhow!(
                "{:#x} bytes at {:#x} cross the allocator's end, {:#x}.", size, curr, self.gpa_end))?;
        let ptr = unsafe { self.host_start.add((curr - self.gpa_start) as usize) };
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len()) };
        self.curr = curr_end;
        Ok(curr)
    }

    fn align(&self, align: u64) -> Result<u64> {
        let aligned = self.curr.checked_next_multiple_of(align)
            .filter(|&a| a <= self.gpa_end)
            .ok_or_else(|| anyhow!(
                "Aligning {:#x} to {:#x} crosses the allocator's end, {:#x}.", self.curr, align, self.gpa_end))?;
        Ok(aligned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocopy::byteorder::{ BE, LE, U16, U32, U64 };

    const BASE: u64 = 0x1000;

    fn alloc(mem: &mut [u8]) -> BumpAllocator {
        BumpAllocator::new(BASE, BASE + mem.len() as u64, mem.as_mut_ptr())
    }

    #[derive(IntoBytes, Immutable)]
    #[repr(C)]
    struct Entry {
        addr: U64<LE>,
        kind: U32<LE>,
        tag: U32<BE>,
    }

    #[test]
    fn push_copies_and_bumps() {
        let mut mem = [0u8; 16];
        let mut a = alloc(&mut mem);
        assert_eq!(a.push(b"abc").unwrap(), BASE);
        assert_eq!(a.push(b"de").unwrap(), BASE + 3);
        assert_eq!(a.push(b"f").unwrap(), BASE + 5);
        assert_eq!(&mem[..6], b"abcdef");
    }

    #[test]
    fn push_aligned_skips_to_the_multiple() {
        let mut mem = [0u8; 32];
        let mut a = alloc(&mut mem);
        a.push(b"a").unwrap();
        assert_eq!(a.push_aligned(b"b", Some(16)).unwrap(), BASE + 16);
        /* Already aligned: no gap. */
        assert_eq!(a.push_aligned(b"c", Some(1)).unwrap(), BASE + 17);
        assert_eq!(&mem[..18], b"a\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0bc");
    }

    #[test]
    fn types_align_themselves() {
        let mut mem = [0u8; 32];
        let mut a = alloc(&mut mem);
        a.push(b"a").unwrap();
        assert_eq!(a.push(&[U16::<LE>::new(0x0102)]).unwrap(), BASE + 1);
        /* U32<LE> has alignment 1, but a native u32 has 4. */
        assert_eq!(a.push(&[0x0304u32]).unwrap(), BASE + 4);
        assert_eq!(a.push(&[0u64]).unwrap(), BASE + 8);
        /* A smaller explicit alignment does not undercut the type's. */
        a.push(b"x").unwrap();
        assert_eq!(a.push_aligned(&[0u64], Some(2)).unwrap(), BASE + 24);
    }

    #[test]
    fn structs_keep_their_byte_order() {
        let mut mem = [0u8; 16];
        let mut a = alloc(&mut mem);
        let e = Entry { addr: U64::new(0x1122_3344_5566_7788), kind: U32::new(1), tag: U32::new(0xaabbccdd) };
        assert_eq!(a.push(&[e]).unwrap(), BASE);
        assert_eq!(mem, [
            0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11,
            0x01, 0x00, 0x00, 0x00,
            0xaa, 0xbb, 0xcc, 0xdd,
        ]);
    }

    #[test]
    fn slices_go_in_back_to_back() {
        let mut mem = [0u8; 8];
        let mut a = alloc(&mut mem);
        let words = [U16::<BE>::new(0x0102), U16::new(0x0304), U16::new(0x0506)];
        assert_eq!(a.push(&words).unwrap(), BASE);
        assert_eq!(&mem[..6], [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn up_to_the_end_fits() {
        let mut mem = [0u8; 16];
        let mut a = alloc(&mut mem);
        a.push(&[0xffu8; 15]).unwrap();
        assert_eq!(a.push(b"z").unwrap(), BASE + 15);
        /* Nothing left, but an empty push still fits. */
        assert_eq!(a.push::<u8>(&[]).unwrap(), BASE + 16);
        assert_eq!(mem[15], b'z');
    }

    #[test]
    fn crossing_the_end_fails_and_changes_nothing() {
        let mut mem = [0u8; 16];
        let mut a = alloc(&mut mem);
        a.push(b"0123456789").unwrap();
        assert!(a.push(b"0123456").is_err());
        assert!(a.push_aligned(b"x", Some(0x20)).is_err());
        assert!(a.push(&[0u64; 1]).is_err());
        assert_eq!(&mem[10..], [0u8; 6]);
        /* What still fits goes right where the failed pushes would have. */
        assert_eq!(a.push(b"abcdef").unwrap(), BASE + 10);
        assert_eq!(&mem[10..], b"abcdef");
    }
}
