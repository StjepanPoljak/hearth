use anyhow::{ anyhow, bail, Context, Result };
use crate::syscall_check;
use libc::{ _IOWR,_IOW };
use crate::{ KvmDev, VCPU, VM, VmConfig };
use crate::{ read_le32, read_le64, open_file };
use crate::{ kvm_create_device, kvm_device_attr, kvm_device_type };
use crate::{ KVM_VGIC_V2_ADDR_TYPE_DIST, KVM_VGIC_V2_ADDR_TYPE_CPU, KVM_DEV_ARM_VGIC_GRP_ADDR };

use std::io::Read;

use crate::KVMIO;

const KVM_CREATE_DEVICE: libc::Ioctl = _IOWR::<kvm_create_device>(KVMIO, 0xe0);
const KVM_SET_DEVICE_ATTR: libc::Ioctl = _IOW::<kvm_device_attr>(KVMIO, 0xe1);

pub fn arch_pre_vcpu_init(_kvm_dev: &KvmDev, _vm: &mut VM) -> Result<()> {
    Ok(())
}

pub fn arch_init(_kvm_dev: &KvmDev, vm: &mut VM, vcpu: &mut VCPU) -> Result<()> {
    vcpu.arm_vcpu_init()?;
    vm.setup_irq_device()
}

#[repr(C)]
struct Arm64ImageHeader {
    code0: u32,
    code1: u32,
    text_offset: u64,   // le
    image_size: u64,    // le
    flags: u64,
    res2: u64,
    res3: u64,
    res4: u64,
    magic: u32,         // 0x644d5241 == "ARM\x64"
    res5: u32,
}

fn get_linux_header(path: &str) -> Result<Arm64ImageHeader> {
    let mut file = open_file(path)?;
    let mut buf = [0u8; 64];
    file.read_exact(&mut buf).with_context(|| format!("Failed to read {}", path))?;
    Ok(Arm64ImageHeader{
        code0: read_le32(&buf, 0),
        code1: read_le32(&buf, 4),
        text_offset: read_le64(&buf, 8),
        image_size: read_le64(&buf, 16),
        flags: read_le64(&buf, 24),
        res2: read_le64(&buf, 32),
        res3: read_le64(&buf, 40),
        res4: read_le64(&buf, 48),
        magic: read_le32(&buf, 56),
        res5: read_le32(&buf, 60)
    })
}

const LOAD_ADDR: u64 = 0x40_000_000;
const INITRAMFS_OFFS: u64 = 0x200_000;
const KERN_OFFS: u64 = 0x200_000 + INITRAMFS_OFFS;

impl VM {
    pub fn setup_irq_device(&mut self) -> Result<()> {
        let mut create_device: kvm_create_device = unsafe { std::mem::zeroed() };

        create_device.type_ = kvm_device_type::KVM_DEV_TYPE_ARM_VGIC_V2;
        create_device.fd = 0xffffffff;
        create_device.flags = 0;

        syscall_check("KVM_CREATE_DEVICE", unsafe {
            libc::ioctl(self.fd, KVM_CREATE_DEVICE, &mut create_device)
        })?;

        let dist_addr : u64 = 0x08_000_000;
        let cpu_addr: u64 = 0x08_010_000;
        let mut dev_attr : kvm_device_attr = unsafe { std::mem::zeroed() };
        dev_attr.group = KVM_DEV_ARM_VGIC_GRP_ADDR;
        dev_attr.attr = KVM_VGIC_V2_ADDR_TYPE_DIST as u64;
        dev_attr.flags = 0;
        dev_attr.addr = &dist_addr as *const u64 as u64;

        syscall_check("KVM_SET_DEVICE_ATTR", unsafe {
            libc::ioctl(create_device.fd as i32, KVM_SET_DEVICE_ATTR, &mut dev_attr)
        })?;

        let mut dev_attr2 : kvm_device_attr = unsafe { std::mem::zeroed() };
        dev_attr2.group = KVM_DEV_ARM_VGIC_GRP_ADDR;
        dev_attr2.attr = KVM_VGIC_V2_ADDR_TYPE_CPU as u64;
        dev_attr2.flags = 0;
        dev_attr2.addr = &cpu_addr as *const u64 as u64;

        syscall_check("KVM_SET_DEVICE_ATTR", unsafe {
            libc::ioctl(create_device.fd as i32, KVM_SET_DEVICE_ATTR, &mut dev_attr2)
        })?;

        Ok(())

    }
    pub fn arch_load_linux(&mut self, vcpu: &mut VCPU, config: &VmConfig) -> Result<()> {
        let linux_path = config.linux.as_ref().ok_or(anyhow!("No Linux image specified."))?;
        let header = get_linux_header(linux_path)?;
        let file_len = std::fs::metadata(linux_path).with_context(|| format!("Failed to read {}", linux_path))?.len() as usize;

        println!("magic {:#x}, image_size: {}, file_size: {}, text_offset: {:#x}\r", header.magic, header.image_size, file_len, header.text_offset);
        /* Must match the memory node in the DTB. */
        let linux_mem_idx = self.add_mem_region(config.memory.unwrap_or(1 << 30), LOAD_ADDR)?;

        match &config.dtb {
            Some(dtb_path) => { self.load_file_to_memory(linux_mem_idx, &dtb_path, 0x0)?; },
            None => { bail!("Could not find device tree blob."); }
        }

        match &config.initramfs {
            Some(initramfs_path) => { self.load_file_to_memory(linux_mem_idx, &initramfs_path, INITRAMFS_OFFS)?; },
            None => ()
        }

        self.load_file_to_memory(linux_mem_idx, linux_path, KERN_OFFS)?;

        vcpu.set_one_reg("x0", LOAD_ADDR)?;
        vcpu.set_one_reg("x1", 0x0)?;
        vcpu.set_one_reg("x2", 0x0)?;
        vcpu.set_one_reg("x3", 0x0)?;

        vcpu.set_one_reg("pc", LOAD_ADDR + KERN_OFFS)?;

        vcpu.print_regs()?;

        Ok(())
    }
}
