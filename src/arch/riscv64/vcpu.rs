use anyhow::{ anyhow, Result };
use crate::syscall_check;
use std::collections::HashMap;
use std::sync::LazyLock;
use crate::{ kvm_one_reg, KVM_REG_RISCV, KVM_REG_SIZE_U64, KVM_REG_RISCV_CORE };
use libc::_IOW;

use crate::KVMIO;
use crate::{ VCPU, Breakpoint };

const KVM_GET_ONE_REG: libc::Ioctl = _IOW::<kvm_one_reg>(KVMIO, 0xab);
const KVM_SET_ONE_REG: libc::Ioctl = _IOW::<kvm_one_reg>(KVMIO, 0xac);

/* struct kvm_riscv_core in order: user_regs_struct (pc, then x1..x31 by
 * ABI name), then mode. A register's ONE_REG index is its position. */
const CORE_REGS: [&str; 33] = [
    "pc", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1",
    "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7",
    "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11",
    "t3", "t4", "t5", "t6", "mode"
];

static REGS: LazyLock<HashMap<&'static str, u64>> = LazyLock::new(|| {
    let base = KVM_REG_RISCV as u64 | KVM_REG_SIZE_U64 as u64 | KVM_REG_RISCV_CORE as u64;
    CORE_REGS.iter().enumerate().map(|(i, name)| (*name, base | i as u64)).collect()
});

impl VCPU {
    pub fn set_breakpoints(&mut self, _breakpoints: Vec<Breakpoint>) -> Result<()> {
        Err(anyhow!("Breakpoints are x86 only so far."))
    }

    /* Never set up, see set_breakpoints. */
    pub fn handle_debug_exit(&mut self) -> Result<()> {
        Err(anyhow!("Unexpected debug exit."))
    }

    pub fn get_one_reg(&mut self, name: &str) -> Result<u64> {
        let mut val : u64 = 0;
        let mut reg : kvm_one_reg = unsafe { std::mem::zeroed() };

        reg.id = *REGS.get(name).ok_or(anyhow!("Invalid register name {}.", name))?;
        reg.addr = &mut val as *mut u64 as u64;

        syscall_check("KVM_GET_ONE_REG", unsafe {
            libc::ioctl(self.fd, KVM_GET_ONE_REG, &mut reg)
        })?;

        Ok(val)
    }

    pub fn set_one_reg(&mut self, name: &str, val: u64) -> Result<()> {
        let mut reg : kvm_one_reg = unsafe { std::mem::zeroed() };

        reg.id = *REGS.get(name).ok_or(anyhow!("Invalid register name {}.", name))?;
        reg.addr = &val as *const u64 as u64;

        syscall_check("KVM_SET_ONE_REG", unsafe {
            libc::ioctl(self.fd, KVM_SET_ONE_REG, &mut reg)
        })?;

        Ok(())
    }

    pub fn set_ip(&mut self, ip: usize) -> Result<()> {
        self.set_one_reg("pc", ip as u64)
    }

    #[allow(dead_code)]
    pub fn get_ip(&mut self) -> Result<usize> {
        Ok(self.get_one_reg("pc")? as usize)
    }

    pub fn print_regs(&mut self) -> Result<()> {
        for name in ["pc", "ra", "sp", "a0", "a1"] {
            let val = self.get_one_reg(name)?;
            println!("{} = {:#x}\r", name, val);
        }

        Ok(())
    }
}
