use anyhow::Result;
use crate::syscall_check;
use crate::{ VM, VCPU, IO, KVMIO, VmConfig, kvm_irq_level, KVM_ARM_IRQ_TYPE_SPI, KVM_ARM_IRQ_TYPE_SHIFT };
use libc::_IOW;

const KVM_IRQ_LINE: libc::Ioctl = _IOW::<kvm_irq_level>(KVMIO, 0x61);
/* SPIs start at GIC interrupt ID 32. */
const GIC_SPI_BASE: u32 = 32;

/* `num` is the SPI number, as used in the device tree interrupts property. */
pub fn arch_update_irq(level: u32, vm_fd: libc::c_int, num: u32) -> Result<()> {
    let mut irq : kvm_irq_level = unsafe { std::mem::zeroed() };
    irq.level = level;
    irq.__bindgen_anon_1.irq = (KVM_ARM_IRQ_TYPE_SPI << KVM_ARM_IRQ_TYPE_SHIFT) | (GIC_SPI_BASE + num);

    syscall_check("KVM_IRQ_LINE", unsafe {
        libc::ioctl(vm_fd, KVM_IRQ_LINE, &mut irq)
    })?;

    Ok(())
}

impl VM {
    pub fn arch_handle_io(&self, _io: &IO, _base: *mut u8) -> Result<()> { Ok(()) } 

    pub fn arch_init_io_devices(&mut self, _vcpu: &VCPU, _config: &VmConfig) -> Result<()> { Ok(()) }
}
