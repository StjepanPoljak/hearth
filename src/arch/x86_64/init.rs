use crate::{ kvm_cpuid2, kvm_cpuid_entry2, kvm_pit_config };
use crate::{ VCPU, VM, KvmDev, VmConfig, KVMIO, PAGE_SIZE };
use crate::{ read_le64, read_le32, load_elf, read_file };
use libc::{_IOW, _IO, _IOWR};

use object::read::elf::{ElfFile64, SectionHeader};
use object::{Endianness, Object};
use std::io;
use super::mmio::{ VIRTIO_BLK_MMIO_ADDR, VIRTIO_BLK_IRQ, VIRTIO_NET_MMIO_ADDR, VIRTIO_NET_IRQ };

const KVM_SET_TSS_ADDR: libc::Ioctl = _IO(KVMIO, 0x47);
const KVM_CREATE_IRQCHIP: libc::Ioctl = _IO(KVMIO, 0x60);
const KVM_CREATE_PIT2: libc::Ioctl = _IOW::<kvm_pit_config>(KVMIO, 0x77);
const KVM_GET_SUPPORTED_CPUID: libc::Ioctl = _IOWR::<kvm_cpuid2>(KVMIO, 0x05);
const KVM_SET_CPUID2: libc::Ioctl = _IOW::<kvm_cpuid2>(KVMIO, 0x90);

#[repr(C)]
struct HvmStartInfo {
    magic: u32,
    version: u32,
    flags: u32,
    nr_modules: u32,
    modlist_paddr: u64,
    cmdline_paddr: u64,
    rsdp_paddr: u64,
    memmap_paddr: u64,
    memmap_entries: u32,
    reserved: u32
}

#[repr(C)]
struct HvmModlistEntry {
    paddr: u64,
    size: u64,
    cmdline_paddr: u64,
    reserved: u64
}

#[repr(C)]
struct HvmMemmapEntry {
    addr: u64,
    size: u64,
    type_: u32,
    reserved: u32
}

/* Three pages in the MMIO hole, clear of RAM and devices, where KVM keeps
 * the TSS it runs real mode with when the host has no unrestricted guest
 * (no EPT, say). The address is QEMU's. */
const TSS_ADDR: u64 = 0xfffb_d000;

/* Where the PVH start info goes, followed by what it points at. */
const HVM_START_INFO_ADDR: u64 = 0x10000;

pub fn arch_pre_vcpu_init(_kvm_dev: &KvmDev, vm: &mut VM) -> io::Result<()> {
    let ret = unsafe { libc::ioctl(vm.fd, KVM_SET_TSS_ADDR, TSS_ADDR) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    let ret = unsafe { libc::ioctl(vm.fd, KVM_CREATE_IRQCHIP, 0x0) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    let pit_config = kvm_pit_config { flags: 0, pad: [0; 15] };

    let ret = unsafe { libc::ioctl(vm.fd, KVM_CREATE_PIT2, &pit_config) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

/* Debug aid, not called: prints the function and index of each CPUID
 * entry, e.g. from arch_init after KVM_GET_SUPPORTED_CPUID. */
#[allow(dead_code)]
unsafe fn print_cpuid(cpuid2: *const kvm_cpuid2) {
    let nent = unsafe { (*cpuid2).nent as usize };
    let entries = unsafe { (*cpuid2).__bindgen_anon_1.entries.as_slice(nent) };
    for entry in entries {
        println!("{:#x} {:#x}", entry.function, entry.index);
    }
}

pub fn arch_init(kvm_dev: &KvmDev, _vm: &mut VM, vcpu: &mut VCPU) -> io::Result<()> {
    let mut cpuid2_buf = vec![0u32; size_of::<kvm_cpuid2>() + 256 * size_of::<kvm_cpuid_entry2>()];
    let cpuid2 = cpuid2_buf.as_mut_ptr() as *mut kvm_cpuid2;
    unsafe { (*cpuid2).nent = 256; }

    let ret = unsafe { libc::ioctl(kvm_dev.fd(), KVM_GET_SUPPORTED_CPUID, cpuid2) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    let ret = unsafe { libc::ioctl(vcpu.fd, KVM_SET_CPUID2, cpuid2) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut sregs2 = vcpu.get_sregs2()?;

    sregs2.cs.base = 0;
    sregs2.cs.selector = 0;

    vcpu.set_sregs2(sregs2)
}

impl VM {
    fn try_pvh_entrypoint(&mut self, vmlinux: &ElfFile64<Endianness>) -> io::Result<u64> {
        let endian = vmlinux.endian();
        let mut notes = vmlinux.section_by_name(".notes").ok_or(io::Error::other("No .notes found."))?
            .elf_section_header()
            .notes(endian, vmlinux.data()).unwrap()
            .unwrap();

        while let Some(note) = notes.next().unwrap() {
            if note.n_type(endian) == object::elf::NoteType(18) {
                return match note.desc().len() {
                    4 => Ok(read_le32(note.desc(), 0) as u64),
                    8 => Ok(read_le64(note.desc(), 0)),
                    _ => Err(io::Error::other(format!("unexpected PHYS32_ENTRY descsz {}", note.desc().len())))
                };
            }
        }

        Err(io::Error::other(format!("No valid PVH entry found.")))
    }

    fn linux_pvh_boot(&mut self, vcpu: &mut VCPU, config: &VmConfig) -> io::Result<()> {
        let vmlinux_path = config.linux.as_ref().unwrap();
        let vmlinux_data = read_file(vmlinux_path)?;
        let vmlinux = ElfFile64::<Endianness>::parse(&*vmlinux_data)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{} is not an ELF file: {}", vmlinux_path, e)))?;

        let pvh_entrypoint = self.try_pvh_entrypoint(&vmlinux)?;

        const EBDA_START: u64 = 0x9fc00;
        const HIMEM_START: u64 = 0x10_0000;
        const MMIO_START: u64 = 0xc000_0000;
        const HIGH_RAM_START: u64 = 0x1_0000_0000;

        /* RAM above MMIO_START moves to 4G so the MMIO hole stays free for devices. */
        let ram_size = config.memory.unwrap_or(1 << 30) as u64;
        let linux_mem_idx = self.add_mem_region(ram_size.min(MMIO_START) as usize, 0x0)?;
        if ram_size > MMIO_START {
            self.add_mem_region((ram_size - MMIO_START) as usize, HIGH_RAM_START)?;
        }

        let last_addr = load_elf(self, &vmlinux, &vmlinux_data, linux_mem_idx)?;

        let mut cmdline = format!("console=ttyS0,115200 root=/dev/vda virtio_mmio.device=4K@{:#x}:{} virtio_mmio.device=4K@{:#x}:{}", VIRTIO_BLK_MMIO_ADDR, VIRTIO_BLK_IRQ, VIRTIO_NET_MMIO_ADDR, VIRTIO_NET_IRQ);
        if let Some(append) = &config.append {
            cmdline = format!("{} {}", cmdline, append);
        }
        /* Boot info goes in low RAM below the EBDA; modules go after the
         * kernel, up to the end of RAM below 4G. */
        let region = self.mem_region_mut(linux_mem_idx)?;
        let boot_alloc = region.add_bump_allocator(HVM_START_INFO_ADDR, EBDA_START)?;
        let module_alloc = region.add_bump_allocator(last_addr.next_multiple_of(PAGE_SIZE), ram_size.min(MMIO_START))?;

        let mut modlist_entries = vec![];
        if let Some(initramfs_path) = &config.initramfs {
            /* The module's command line carries its file name, for guests
             * that look modules up by name; Linux ignores it. */
            let module_name = std::path::Path::new(initramfs_path)
                .file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
            let initramfs = read_file(initramfs_path)?;

            let modules = &mut region.allocators[module_alloc];
            let cmdline_paddr = modules.push(format!("{}\0", module_name).as_bytes())?;
            modules.align(PAGE_SIZE)?;
            println!("Loading {:?} at {:#x}...\r", initramfs_path, modules.next());
            let paddr = modules.push(&initramfs)?;
            modlist_entries.push(HvmModlistEntry { paddr, size: initramfs.len() as u64, cmdline_paddr, reserved: 0 });
        }

        let mut memmap_entries = vec![
            HvmMemmapEntry { addr: 0x0, size: EBDA_START, type_: 1, reserved: 0 },
            HvmMemmapEntry { addr: EBDA_START, size: HIMEM_START - EBDA_START, type_: 2, reserved: 0 },
            HvmMemmapEntry { addr: HIMEM_START, size: ram_size.min(MMIO_START) - HIMEM_START, type_: 1, reserved: 0}
        ];
        if ram_size > MMIO_START {
            memmap_entries.push(HvmMemmapEntry { addr: HIGH_RAM_START, size: ram_size - MMIO_START, type_: 1, reserved: 0 });
        }

        let memmap_size = memmap_entries.len() * std::mem::size_of::<HvmMemmapEntry>();
        let mut memmap_buf = Vec::with_capacity(memmap_size);
        for e in &memmap_entries {
            memmap_buf.extend_from_slice(&e.addr.to_le_bytes());
            memmap_buf.extend_from_slice(&e.size.to_le_bytes());
            memmap_buf.extend_from_slice(&e.type_.to_le_bytes());
            memmap_buf.extend_from_slice(&e.reserved.to_le_bytes());
        }

        let modlist_size = modlist_entries.len() * std::mem::size_of::<HvmModlistEntry>();
        let mut modlist_buf = Vec::with_capacity(modlist_size);
        for e in &modlist_entries {
            modlist_buf.extend_from_slice(&e.paddr.to_le_bytes());
            modlist_buf.extend_from_slice(&e.size.to_le_bytes());
            modlist_buf.extend_from_slice(&e.cmdline_paddr.to_le_bytes());
            modlist_buf.extend_from_slice(&e.reserved.to_le_bytes());
        }

        /* The start info points at the rest, so it is filled in last. */
        let boot = &mut region.allocators[boot_alloc];
        let start_info_paddr = boot.next();
        let start_info = boot.reserve(std::mem::size_of::<HvmStartInfo>())?;
        boot.align(PAGE_SIZE)?;
        let cmdline_start = boot.push(format!("{}\0", cmdline).as_bytes())?;
        boot.align(PAGE_SIZE)?;
        let memmap_start = boot.push(&memmap_buf)?;
        boot.align(PAGE_SIZE)?;
        let modlist_start = boot.push(&modlist_buf)?;

        let mut magic = [ 'x', 'E', 'n', '3' ].map(|c| c as u8); magic[1] |= 0x80;
        let hvm_start_info = HvmStartInfo {
            magic: read_le32(&magic, 0x0),
            version: 1u32,
            flags: 0u32,
            nr_modules: modlist_entries.len() as u32,
            modlist_paddr: modlist_start as u64,
            cmdline_paddr: cmdline_start as u64,
            rsdp_paddr: 0u64,
            memmap_paddr: memmap_start as u64,
            memmap_entries: memmap_entries.len() as u32,
            reserved: 0u32
        };
        unsafe { std::ptr::write_unaligned(start_info.cast::<HvmStartInfo>(), hvm_start_info) };

        let mut regs = vcpu.get_regs()?;
        regs.rip = pvh_entrypoint;
        regs.rbx = start_info_paddr;
        regs.rflags = 0x2; // bits 17, 9 and 8 cleared, all other unspecified
        vcpu.set_regs(regs)?;
        regs.print();

        vcpu.protected_mode_setup()?;

        Ok(())
    }

    pub fn arch_load_linux(&mut self, vcpu: &mut VCPU, config: &VmConfig) -> io::Result<()> {
        self.linux_pvh_boot(vcpu, config)
    }
}
