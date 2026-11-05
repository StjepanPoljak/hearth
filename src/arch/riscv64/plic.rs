use anyhow::Result;
use crate::syscall_check;
use crate::{ MMIO, MMIODevice, GuestMemory, KVMIO, read_le32, write_le32 };
use crate::{ kvm_interrupt, KVM_INTERRUPT_SET, KVM_INTERRUPT_UNSET };
use libc::_IOW;
use std::sync::{ Arc, Mutex, LazyLock };

const KVM_INTERRUPT: libc::Ioctl = _IOW::<kvm_interrupt>(KVMIO, 0x86);

/* QEMU virt's PLIC address. One context, the hart's S-mode one: the device
 * tree gives it as interrupts-extended = <&cpu0_intc 9>. */
pub const PLIC_MMIO_ADDR: u64 = 0xc00_0000;
pub const PLIC_MMIO_SIZE: u64 = 0x40_0000;
/* Sources 1 to 31 (riscv,ndev = 31); 0 means none. */
const PLIC_SOURCES: usize = 32;

const PRIORITY_BASE: u64 = 0x0;
const PENDING_BASE: u64 = 0x1000;
const ENABLE_BASE: u64 = 0x2000;
const THRESHOLD: u64 = 0x20_0000;
const CLAIM_COMPLETE: u64 = 0x20_0004;

/* Level-triggered gateways: a source is pending while its line is high,
 * unless it's claimed and not yet completed. The hart's external interrupt
 * is up while an enabled source is pending above the threshold. */
pub struct Plic {
    vcpu_fd: libc::c_int,
    level: u32,
    pending: u32,
    claimed: u32,
    enable: u32,
    priority: [u32; PLIC_SOURCES],
    threshold: u32,
    eip: bool,
}

pub static PLIC: LazyLock<Arc<Mutex<Plic>>> = LazyLock::new(|| {
    Arc::new(Mutex::new(Plic {
        vcpu_fd: -1, level: 0, pending: 0, claimed: 0, enable: 0,
        priority: [0; PLIC_SOURCES], threshold: 0, eip: false
    }))
});

impl Plic {
    /* Interrupts raised before there's a vcpu are delivered from here. */
    pub fn set_vcpu(&mut self, vcpu_fd: libc::c_int) -> Result<()> {
        self.vcpu_fd = vcpu_fd;
        self.eip = false;
        self.update()
    }

    pub fn set_level(&mut self, irq: u32, level: bool) -> Result<()> {
        let bit = Self::bit(irq);
        if level {
            self.level |= bit;
            if self.claimed & bit == 0 {
                self.pending |= bit;
            }
        } else {
            self.level &= !bit;
            self.pending &= !bit;
        }
        self.update()
    }

    fn bit(irq: u32) -> u32 {
        if irq > 0 && (irq as usize) < PLIC_SOURCES { 1 << irq } else { 0 }
    }

    /* The enabled pending source of highest priority above the threshold;
     * on a tie the lowest id. */
    fn best(&self) -> u32 {
        let mut best = 0;
        let mut best_prio = self.threshold;
        for irq in 1..PLIC_SOURCES as u32 {
            let prio = self.priority[irq as usize];
            if self.pending & self.enable & (1 << irq) != 0 && prio > best_prio {
                best = irq;
                best_prio = prio;
            }
        }
        best
    }

    fn update(&mut self) -> Result<()> {
        let eip = self.best() != 0;
        if eip == self.eip || self.vcpu_fd < 0 {
            return Ok(());
        }
        self.eip = eip;
        let mut irq = kvm_interrupt { irq: if eip { KVM_INTERRUPT_SET as u32 } else { KVM_INTERRUPT_UNSET as u32 } };
        syscall_check("KVM_INTERRUPT", unsafe {
            libc::ioctl(self.vcpu_fd, KVM_INTERRUPT, &mut irq)
        })?;
        Ok(())
    }

    fn claim(&mut self) -> u32 {
        let irq = self.best();
        if irq != 0 {
            self.pending &= !(1 << irq);
            self.claimed |= 1 << irq;
        }
        irq
    }

    fn complete(&mut self, irq: u32) {
        let bit = Self::bit(irq);
        self.claimed &= !bit;
        if self.level & bit != 0 {
            self.pending |= bit;
        }
    }

    fn read(&mut self, offset: u64) -> u32 {
        match offset {
            o if o < PRIORITY_BASE + 4 * PLIC_SOURCES as u64 => self.priority[(o / 4) as usize],
            PENDING_BASE => self.pending,
            ENABLE_BASE => self.enable,
            THRESHOLD => self.threshold,
            CLAIM_COMPLETE => self.claim(),
            _ => 0
        }
    }

    fn write(&mut self, offset: u64, val: u32) {
        match offset {
            /* Source 0 doesn't exist, so its priority stays 0. */
            o if o >= 4 && o < PRIORITY_BASE + 4 * PLIC_SOURCES as u64 => self.priority[(o / 4) as usize] = val & 7,
            ENABLE_BASE => self.enable = val & !1,
            THRESHOLD => self.threshold = val & 7,
            CLAIM_COMPLETE => self.complete(val),
            _ => ()
        }
    }
}

impl MMIODevice for Plic {
    fn handle(&mut self, offset: u64, mmio: &mut MMIO, _mem: &GuestMemory) -> Result<()> {
        /* Registers are 32 bits wide and aligned. */
        let offset = offset & !3;
        if mmio.is_write != 0 {
            self.write(offset, read_le32(&mmio.data, 0));
        } else {
            mmio.data = [0; 8];
            let val = self.read(offset);
            write_le32(&mut mmio.data, 0, val);
        }
        self.update()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plic() -> Plic {
        let mut p = Plic { vcpu_fd: -1, level: 0, pending: 0, claimed: 0, enable: 0,
                           priority: [0; PLIC_SOURCES], threshold: 0, eip: false };
        p.priority[1] = 1;
        p.priority[10] = 1;
        p.enable = (1 << 1) | (1 << 10);
        p
    }

    #[test]
    fn claim_complete_level() {
        let mut p = plic();
        p.set_level(10, true).unwrap();
        assert_eq!(p.claim(), 10);
        /* Claimed: no longer offered, even with the line high. */
        assert_eq!(p.best(), 0);
        p.complete(10);
        /* Still high, so pending again. */
        assert_eq!(p.claim(), 10);
        p.set_level(10, false).unwrap();
        p.complete(10);
        assert_eq!(p.claim(), 0);
    }

    #[test]
    fn priority_threshold_enable() {
        let mut p = plic();
        p.priority[10] = 3;
        p.set_level(1, true).unwrap();
        p.set_level(10, true).unwrap();
        assert_eq!(p.best(), 10);
        p.threshold = 3;
        assert_eq!(p.best(), 0);
        p.threshold = 0;
        p.enable = 1 << 1;
        assert_eq!(p.best(), 1);
    }

    #[test]
    fn lowered_before_claim() {
        let mut p = plic();
        p.set_level(1, true).unwrap();
        p.set_level(1, false).unwrap();
        assert_eq!(p.claim(), 0);
    }
}
