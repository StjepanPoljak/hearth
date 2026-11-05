use anyhow::{ bail, Result };
use crate::{ VM, VCPU, IO, VmConfig };

/* RISC-V KVM has no in-kernel irqchip except AIA, which needs the host
 * to have Ssaia. The portable way is a PLIC emulated here, raising the
 * vcpu's external interrupt with KVM_INTERRUPT, as kvmtool does. Until
 * then a raised line is an error rather than a lost interrupt; lowering
 * one is trivially done. */
pub fn arch_update_irq(level: u32, _vm_fd: libc::c_int, num: u32) -> Result<()> {
    if level != 0 {
        bail!("Interrupt {} raised, but riscv64 has no interrupt controller yet.", num);
    }
    Ok(())
}

impl VM {
    pub fn arch_handle_io(&self, _io: &IO, _base: *mut u8) -> Result<()> { Ok(()) }

    pub fn arch_init_io_devices(&mut self, _vcpu: &VCPU, _config: &VmConfig) -> Result<()> { Ok(()) }
}
