use anyhow::{ anyhow, bail, Context, Result };
use crate::{ KvmDev, VCPU, VM, VmConfig };
use crate::{ read_le32, read_le64, open_file };

use super::plic::{ PLIC, PLIC_MMIO_ADDR };

use std::io::Read;

pub fn arch_pre_vcpu_init(_kvm_dev: &KvmDev, _vm: &mut VM) -> Result<()> {
    Ok(())
}

/* RISC-V vcpus need no init ioctl. The PLIC interrupts the one vcpu. */
pub fn arch_init(_kvm_dev: &KvmDev, _vm: &mut VM, vcpu: &mut VCPU) -> Result<()> {
    PLIC.lock().unwrap().set_vcpu(vcpu.fd)
}

/* Documentation/arch/riscv/boot-image-header.rst */
const RISCV_IMAGE_MAGIC2: u32 = 0x05435352; // "RSC\x05"

struct RiscvImageHeader {
    text_offset: u64,
    image_size: u64,
    magic2: u32,
}

fn get_linux_header(path: &str) -> Result<RiscvImageHeader> {
    let mut file = open_file(path)?;
    let mut buf = [0u8; 64];
    file.read_exact(&mut buf).with_context(|| format!("Failed to read {}", path))?;
    Ok(RiscvImageHeader {
        text_offset: read_le64(&buf, 8),
        image_size: read_le64(&buf, 16),
        magic2: read_le32(&buf, 56),
    })
}

/* QEMU virt's RAM base. Linux's RAM starts here: the device tree at the
 * base, the kernel 2M in (rv64 wants it 2M aligned), the initramfs at
 * 128M. The DTB's memory node and linux,initrd-start/end must match. */
const LOAD_ADDR: u64 = 0x8000_0000;
const KERN_OFFS: u64 = 0x20_0000;
const INITRAMFS_OFFS: u64 = 0x800_0000;

/* Where the devices start: the PLIC, then the UART and virtio (see
 * mmio.rs). */
const DEVICES_START: u64 = PLIC_MMIO_ADDR;

impl VM {
    /* Guest RAM from 0, for --binary; it must end before the devices.
     * (Linux gets its RAM at LOAD_ADDR instead.) */
    pub fn add_ram(&mut self, size: usize) -> Result<usize> {
        if size as u64 > DEVICES_START {
            bail!("--memory {:#x} is too big for --binary: RAM from 0 must end below the devices at {:#x}.", size, DEVICES_START);
        }
        self.add_mem_region(size, 0x0)
    }

    pub fn arch_load_linux(&mut self, vcpu: &mut VCPU, config: &VmConfig) -> Result<()> {
        let linux_path = config.linux.as_ref().ok_or(anyhow!("No Linux image specified."))?;
        let header = get_linux_header(linux_path)?;
        if header.magic2 != RISCV_IMAGE_MAGIC2 {
            bail!("{} is not a RISC-V Linux Image (magic2 {:#x}).", linux_path, header.magic2);
        }
        println!("image_size: {}, text_offset: {:#x}\r", header.image_size, header.text_offset);

        let mem_size = config.memory.unwrap_or(1 << 30);
        if config.initramfs.is_some() && (mem_size as u64) <= INITRAMFS_OFFS {
            bail!("--memory {:#x} is too small: the initramfs goes at {:#x} into RAM.", mem_size, INITRAMFS_OFFS);
        }
        /* Must match the memory node in the DTB. */
        let linux_mem_idx = self.add_mem_region(mem_size, LOAD_ADDR)?;

        match &config.dtb {
            Some(dtb_path) => { self.load_file_to_memory(linux_mem_idx, dtb_path, 0x0)?; },
            None => { bail!("Could not find device tree blob."); }
        }

        if let Some(initramfs_path) = &config.initramfs {
            self.load_file_to_memory(linux_mem_idx, initramfs_path, INITRAMFS_OFFS)?;
        }

        self.load_file_to_memory(linux_mem_idx, linux_path, KERN_OFFS)?;

        /* The SBI boot protocol: a0 the hart id, a1 the device tree. */
        vcpu.set_one_reg("a0", 0)?;
        vcpu.set_one_reg("a1", LOAD_ADDR)?;
        vcpu.set_one_reg("pc", LOAD_ADDR + KERN_OFFS)?;

        vcpu.print_regs()?;

        Ok(())
    }
}
