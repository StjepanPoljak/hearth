use anyhow::Result;
use crate::{ VM, VCPU, IO, VmConfig };
use super::plic::PLIC;

/* RISC-V KVM has no in-kernel irqchip except AIA, which needs the host
 * to have Ssaia, so the PLIC is emulated here (see plic.rs). `num` is
 * the PLIC source, as in the device tree's interrupts property. */
pub fn arch_update_irq(level: u32, _vm_fd: libc::c_int, num: u32) -> Result<()> {
    PLIC.lock().unwrap().set_level(num, level != 0)
}

impl VM {
    pub fn arch_handle_io(&self, _io: &IO, _base: *mut u8) -> Result<()> { Ok(()) }

    pub fn arch_init_io_devices(&mut self, _vcpu: &VCPU, _config: &VmConfig) -> Result<()> { Ok(()) }
}
