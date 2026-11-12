use anyhow::{ anyhow, Context, Result };
use crate::syscall_check;
use crate::{ kvm_cpuid2, kvm_cpuid_entry2, kvm_pit_config };
use crate::{ VCPU, VM, KvmDev, VmConfig, KVMIO, PAGE_SIZE };
use crate::{ read_le64, read_le32, load_elf, read_file };
use libc::{_IOW, _IO, _IOWR};
use zerocopy::{ Immutable, IntoBytes };
use zerocopy::byteorder::{ LE, U32, U64 };

use object::read::elf::{ElfFile64, SectionHeader};
use object::{Endianness, Object};
use super::mmio::{ VIRTIO_BLK_MMIO_ADDR, VIRTIO_BLK_IRQ, VIRTIO_NET_MMIO_ADDR, VIRTIO_NET_IRQ };

const KVM_SET_TSS_ADDR: libc::Ioctl = _IO(KVMIO, 0x47);
const KVM_CREATE_IRQCHIP: libc::Ioctl = _IO(KVMIO, 0x60);
const KVM_CREATE_PIT2: libc::Ioctl = _IOW::<kvm_pit_config>(KVMIO, 0x77);
const KVM_GET_SUPPORTED_CPUID: libc::Ioctl = _IOWR::<kvm_cpuid2>(KVMIO, 0x05);
const KVM_SET_CPUID2: libc::Ioctl = _IOW::<kvm_cpuid2>(KVMIO, 0x90);

/* PVH boot structures. Their fields are little-endian whatever the host,
 * and align(8) keeps the alignment the u64 fields would have had. */
#[derive(IntoBytes, Immutable)]
#[repr(C, align(8))]
struct HvmStartInfo {
    magic: U32<LE>,
    version: U32<LE>,
    flags: U32<LE>,
    nr_modules: U32<LE>,
    modlist_paddr: U64<LE>,
    cmdline_paddr: U64<LE>,
    rsdp_paddr: U64<LE>,
    memmap_paddr: U64<LE>,
    memmap_entries: U32<LE>,
    reserved: U32<LE>
}

#[derive(IntoBytes, Immutable)]
#[repr(C, align(8))]
struct HvmModlistEntry {
    paddr: U64<LE>,
    size: U64<LE>,
    cmdline_paddr: U64<LE>,
    reserved: U64<LE>
}

#[derive(IntoBytes, Immutable)]
#[repr(C, align(8))]
struct HvmMemmapEntry {
    addr: U64<LE>,
    size: U64<LE>,
    type_: U32<LE>,
    reserved: U32<LE>
}

const E820_RAM: u32 = 1;
const E820_RESERVED: u32 = 2;

impl HvmStartInfo {
    fn new(nr_modules: u32, modlist_paddr: u64, cmdline_paddr: u64,
           memmap_paddr: u64, memmap_entries: u32) -> Self {
        /* "xEn3" with the top bit of 'E' set. */
        let mut magic = [ 'x', 'E', 'n', '3' ].map(|c| c as u8); magic[1] |= 0x80;
        HvmStartInfo {
            magic: U32::from_bytes(magic),
            version: 1.into(),
            flags: 0.into(),
            nr_modules: nr_modules.into(),
            modlist_paddr: modlist_paddr.into(),
            cmdline_paddr: cmdline_paddr.into(),
            rsdp_paddr: 0.into(),
            memmap_paddr: memmap_paddr.into(),
            memmap_entries: memmap_entries.into(),
            reserved: 0.into()
        }
    }
}

impl HvmModlistEntry {
    fn new(paddr: u64, size: u64, cmdline_paddr: u64) -> Self {
        HvmModlistEntry { paddr: paddr.into(), size: size.into(), cmdline_paddr: cmdline_paddr.into(), reserved: 0.into() }
    }
}

impl HvmMemmapEntry {
    fn new(addr: u64, size: u64, type_: u32) -> Self {
        HvmMemmapEntry { addr: addr.into(), size: size.into(), type_: type_.into(), reserved: 0.into() }
    }
}

/* Three pages in the MMIO hole, clear of RAM and devices, where KVM keeps
 * the TSS it runs real mode with when the host has no unrestricted guest
 * (no EPT, say). The address is QEMU's. */
const TSS_ADDR: u64 = 0xfffb_d000;

/* The hole below 4G that RAM leaves free, for the devices (virtio at
 * 0xd0000000) and KVM's TSS. */
const MMIO_START: u64 = 0xc000_0000;
const HIGH_RAM_START: u64 = 0x1_0000_0000;

impl VM {
    /* Guest RAM from 0; what doesn't fit below MMIO_START goes at 4G, so
     * the hole stays free. Returns the low region, which holds address 0. */
    pub fn add_ram(&mut self, size: usize) -> Result<usize> {
        let low_idx = self.add_mem_region(size.min(MMIO_START as usize), 0x0)?;
        if size as u64 > MMIO_START {
            self.add_mem_region(size - MMIO_START as usize, HIGH_RAM_START)?;
        }
        Ok(low_idx)
    }
}

/* Where the PVH start info goes, followed by what it points at. */
const HVM_START_INFO_ADDR: u64 = 0x10000;

pub fn arch_pre_vcpu_init(_kvm_dev: &KvmDev, vm: &mut VM) -> Result<()> {
    syscall_check("KVM_SET_TSS_ADDR", unsafe { libc::ioctl(vm.fd, KVM_SET_TSS_ADDR, TSS_ADDR) })?;

    syscall_check("KVM_CREATE_IRQCHIP", unsafe { libc::ioctl(vm.fd, KVM_CREATE_IRQCHIP, 0x0) })?;

    let pit_config = kvm_pit_config { flags: 0, pad: [0; 15] };

    syscall_check("KVM_CREATE_PIT2", unsafe { libc::ioctl(vm.fd, KVM_CREATE_PIT2, &pit_config) })?;

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

pub fn arch_init(kvm_dev: &KvmDev, _vm: &mut VM, vcpu: &mut VCPU) -> Result<()> {
    let mut cpuid2_buf = vec![0u32; size_of::<kvm_cpuid2>() + 256 * size_of::<kvm_cpuid_entry2>()];
    let cpuid2 = cpuid2_buf.as_mut_ptr() as *mut kvm_cpuid2;
    unsafe { (*cpuid2).nent = 256; }

    syscall_check("KVM_GET_SUPPORTED_CPUID", unsafe { libc::ioctl(kvm_dev.fd(), KVM_GET_SUPPORTED_CPUID, cpuid2) })?;

    syscall_check("KVM_SET_CPUID2", unsafe { libc::ioctl(vcpu.fd, KVM_SET_CPUID2, cpuid2) })?;

    let mut sregs2 = vcpu.get_sregs2()?;

    sregs2.cs.base = 0;
    sregs2.cs.selector = 0;

    vcpu.set_sregs2(sregs2)
}

impl VM {
    fn try_pvh_entrypoint(&mut self, vmlinux: &ElfFile64<Endianness>) -> Result<u64> {
        let endian = vmlinux.endian();
        let mut notes = vmlinux.section_by_name(".notes").ok_or(anyhow!("No .notes found."))?
            .elf_section_header()
            .notes(endian, vmlinux.data()).unwrap()
            .unwrap();

        while let Some(note) = notes.next().unwrap() {
            if note.n_type(endian) == object::elf::NoteType(18) {
                return match note.desc().len() {
                    4 => Ok(read_le32(note.desc(), 0) as u64),
                    8 => Ok(read_le64(note.desc(), 0)),
                    _ => Err(anyhow!("unexpected PHYS32_ENTRY descsz {}", note.desc().len()))
                };
            }
        }

        Err(anyhow!("No valid PVH entry found."))
    }

    fn linux_pvh_boot(&mut self, vcpu: &mut VCPU, config: &VmConfig) -> Result<()> {
        let vmlinux_path = config.linux.as_ref().unwrap();
        let vmlinux_data = read_file(vmlinux_path)?;
        let vmlinux = ElfFile64::<Endianness>::parse(&*vmlinux_data)
            .with_context(|| format!("{} is not an ELF file", vmlinux_path))?;

        let pvh_entrypoint = self.try_pvh_entrypoint(&vmlinux)?;

        const EBDA_START: u64 = 0x9fc00;
        const HIMEM_START: u64 = 0x10_0000;

        let ram_size = config.memory.unwrap_or(1 << 30) as u64;
        let linux_mem_idx = self.add_ram(ram_size as usize)?;

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
            let paddr = modules.push_aligned(&initramfs, Some(PAGE_SIZE))?;
            println!("Loaded {:?} at {:#x}.\r", initramfs_path, paddr);
            modlist_entries.push(HvmModlistEntry::new(paddr, initramfs.len() as u64, cmdline_paddr));
        }

        let mut memmap_entries = vec![
            HvmMemmapEntry::new(0x0, EBDA_START, E820_RAM),
            HvmMemmapEntry::new(EBDA_START, HIMEM_START - EBDA_START, E820_RESERVED),
            HvmMemmapEntry::new(HIMEM_START, ram_size.min(MMIO_START) - HIMEM_START, E820_RAM)
        ];
        if ram_size > MMIO_START {
            memmap_entries.push(HvmMemmapEntry::new(HIGH_RAM_START, ram_size - MMIO_START, E820_RAM));
        }

        /* The start info points at the rest, so it is filled in last. */
        let boot = &mut region.allocators[boot_alloc];
        let cmdline_paddr = boot.push(format!("{}\0", cmdline).as_bytes())?;
        let memmap_paddr = boot.push_aligned(&memmap_entries, Some(PAGE_SIZE))?;
        let modlist_paddr = boot.push_aligned(&modlist_entries, Some(PAGE_SIZE))?;

        let hvm_start_info = HvmStartInfo::new(
            modlist_entries.len() as u32, modlist_paddr, cmdline_paddr,
            memmap_paddr, memmap_entries.len() as u32);
        let start_info_paddr = boot.push_aligned(&[hvm_start_info], Some(PAGE_SIZE))?;

        let mut regs = vcpu.get_regs()?;
        regs.rip = pvh_entrypoint;
        regs.rbx = start_info_paddr;
        regs.rflags = 0x2; // bits 17, 9 and 8 cleared, all other unspecified
        vcpu.set_regs(regs)?;
        regs.print();

        vcpu.protected_mode_setup()?;

        Ok(())
    }

    pub fn arch_load_linux(&mut self, vcpu: &mut VCPU, config: &VmConfig) -> Result<()> {
        self.linux_pvh_boot(vcpu, config)
    }
}
