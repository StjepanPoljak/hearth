use anyhow::{ anyhow, Context, Result };
use crate::syscall_check;
use std::collections::HashMap;
use std::sync::LazyLock;
use crate::{ kvm_vcpu_init, kvm_one_reg, KVM_REG_ARM64, KVM_REG_SIZE_U64, KVM_REG_ARM_CORE };
use libc::_IOW;

use crate::KVMIO;
use crate::{ VCPU, Breakpoint };

const KVM_ARM_VCPU_INIT: libc::Ioctl = _IOW::<kvm_vcpu_init>(KVMIO, 0xae);
const KVM_GET_ONE_REG: libc::Ioctl = _IOW::<kvm_one_reg>(KVMIO, 0xab);
const KVM_SET_ONE_REG: libc::Ioctl = _IOW::<kvm_one_reg>(KVMIO, 0xac);

fn aarch64_core_reg(name: &str) -> Result<u64> {
    let base = KVM_REG_ARM64 | KVM_REG_SIZE_U64 | KVM_REG_ARM_CORE as u64;
    if name.starts_with("x") {
        let reg : u64 = name[1..]
            .parse()
            .with_context(|| format!("invalid register {}", name))?;
        return Ok(base | reg * 2); }

    match name {
        "sp" => { return Ok(base | (31 * 2)); },
        "pc" => { return Ok(base | (32 * 2)); },
        "pstate" => { return Ok(base | (33 * 2)); },
        _ => ()
    };

    Err(anyhow!("Invalid register."))
}

fn load_regs_hash() -> Result<HashMap<String, u64>> {
    let mut res = HashMap::<String, u64>::new();
    let mut regs = (0..30).map(|x| format!("x{x}")).collect::<Vec<String>>();
    regs.extend(["sp", "pc", "pstate"].iter().map(|x| x.to_string()).collect::<Vec<String>>());
    for each in regs {
        let id = aarch64_core_reg(&each)?;
        res.insert(each, id);
    }
    Ok(res)
}

static REGS: LazyLock<HashMap<String, u64>> = LazyLock::new(|| {
    load_regs_hash().expect("Failed to load register hash.")
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

        reg.id = *REGS.get(name).ok_or(anyhow!("Invalid register name."))?;
        reg.addr = &mut val as *mut u64 as u64;

        syscall_check("KVM_GET_ONE_REG", unsafe {
            libc::ioctl(self.fd, KVM_GET_ONE_REG, &mut reg)
        })?;

        Ok(val)
    }

    pub fn set_one_reg(&mut self, name: &str, val: u64) -> Result<()> {
        let mut reg : kvm_one_reg = unsafe { std::mem::zeroed() };

        reg.id = *REGS.get(name).ok_or(anyhow!("Invalid register name."))?;
        reg.addr = &val as *const u64 as u64;

        syscall_check("KVM_SET_ONE_REG", unsafe {
            libc::ioctl(self.fd, KVM_SET_ONE_REG, &mut reg)
        })?;

        Ok(())
    }

    pub fn arm_vcpu_init(&self) -> Result<()> {
        let mut vcpu_init : kvm_vcpu_init = unsafe { std::mem::zeroed() };
        // KVM_ARM_VCPU_INIT: target=5 features=(0, 0, 0, 0, 0, 0, 0)
        // KVM_ARM_VCPU_INIT: target=5 features=(12, 0, 0, 0, 0, 0, 0)
        // KVM_ARM_VCPU_INIT: target=5 features=(12, 0, 0, 0, 0, 0, 0)
        // KVM_ARM_VCPU_INIT: target=5 features=(12, 0, 0, 0, 0, 0, 0)
        vcpu_init.target = 5;
        syscall_check("KVM_ARM_VCPU_INIT", unsafe { libc::ioctl(self.fd, KVM_ARM_VCPU_INIT, &mut vcpu_init) })?;

        Ok(())
    }

    pub fn set_ip(&mut self, ip: usize) -> Result<()> {
        self.set_one_reg("pc", ip as u64)
    }

    pub fn get_ip(&mut self) -> Result<usize> {
        let pc = self.get_one_reg("x0")?;
        Ok(pc as usize)
    }

    pub fn print_regs(&mut self) -> Result<()> {
        let x0 = self.get_one_reg("x0")?;
        println!("x0 = {:#x}\r", x0);
        let pc = self.get_one_reg("pc")?;
        println!("pc = {:#x}\r", pc);

        Ok(())
    }
}
