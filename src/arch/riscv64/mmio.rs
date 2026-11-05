use anyhow::Result;
use crate::{ VM, MMIO, MMIODevice, GuestMemory, TTY, VmControl, VmConfig, SerialSpec, SerialOut };
use crate::{ start_serial, VirtioPlaceholder, VIRTIO_MMIO_SIZE };
use super::arch_update_irq;
use super::plic::{ PLIC, PLIC_MMIO_ADDR, PLIC_MMIO_SIZE };
use std::collections::VecDeque;
use std::sync::{ Arc, Mutex };

/* QEMU's riscv virt machine layout, so its device trees describe these:
 * an ns16550a (reg-shift 0) on PLIC interrupt 10, and 8 virtio-mmio slots
 * from 0x10001000, 0x1000 apart, on interrupts 1 to 8. virtio-blk takes
 * the first; the guest probes the others too, so they hold empty
 * placeholders. */
const UART_MMIO_ADDR: u64 = 0x1000_0000;
const UART_MMIO_SIZE: u64 = 0x100;
const UART_IRQ: u32 = 10;
const VIRTIO_MMIO_BASE: u64 = 0x1000_1000;
const VIRTIO_MMIO_STRIDE: u64 = 0x1000;
const VIRTIO_MMIO_SLOTS: u64 = 8;
const VIRTIO_BLK_IRQ: u32 = 1;

/* A 16550 like x86's (arch/x86_64/io.rs), on MMIO: registers 0 to 7. */
pub struct UART {
    ier: u8,
    fcr: u8,
    lcr: u8,
    mcr: u8,
    scr: u8,
    dll: u8,
    dlm: u8,
    thr_ipending: bool,
    irq_level: bool,
    vm_fd: libc::c_int,
    rx: VecDeque<u8>,
    out: SerialOut
}

impl TTY for UART {
    fn send_char(&mut self, ch: u8) -> Result<()> {
        self.rx.push_back(ch);
        self.update_irq()
    }
}

impl UART {
    fn new(vm_fd: libc::c_int, control: Arc<VmControl>, serial: &SerialSpec) -> Result<Arc<Mutex<Self>>> {
        let uart = Arc::new(Mutex::new(UART {
            ier: 0, fcr: 0, lcr: 0, mcr: 0, scr: 0, dll: 0, dlm: 0,
            thr_ipending: false, irq_level: false, vm_fd, rx: VecDeque::new(),
            out: SerialOut::default()
        }));

        let out = start_serial(serial, uart.clone(), control)?;
        uart.lock().unwrap().out = out;
        Ok(uart)
    }

    fn dlab(&self) -> bool { self.lcr & 0x80 != 0 }

    /* No OUT2 gating here, unlike on a PC: device tree 8250s are wired
     * straight to the interrupt controller. */
    fn update_irq(&mut self) -> Result<()> {
        let level = (self.ier & 0x01 != 0 && !self.rx.is_empty())
            || (self.ier & 0x02 != 0 && self.thr_ipending);

        if level != self.irq_level {
            self.irq_level = level;
            arch_update_irq(level as u32, self.vm_fd, UART_IRQ)?;
        }
        Ok(())
    }

    fn read(&mut self, reg: u64) -> u8 {
        match reg {
            0 if self.dlab() => self.dll,
            0 => self.rx.pop_front().unwrap_or(0),
            1 if self.dlab() => self.dlm,
            1 => self.ier,
            2 => {
                let fifo = if self.fcr & 0x01 != 0 { 0xc0 } else { 0x00 };
                if self.ier & 0x01 != 0 && !self.rx.is_empty() {
                    fifo | 0x04
                } else if self.ier & 0x02 != 0 && self.thr_ipending {
                    self.thr_ipending = false;
                    fifo | 0x02
                } else {
                    fifo | 0x01
                }
            },
            3 => self.lcr,
            4 => self.mcr,
            5 => if self.rx.is_empty() { 0x60 } else { 0x61 },
            6 => if self.mcr & 0x10 != 0 {
                    ((self.mcr & 0x02) << 3) | ((self.mcr & 0x01) << 5)
                  | ((self.mcr & 0x04) << 4) | ((self.mcr & 0x08) << 4)
                } else { 0xb0 },
            7 => self.scr,
            _ => 0
        }
    }

    fn write(&mut self, reg: u64, val: u8) {
        match reg {
            0 if self.dlab() => self.dll = val,
            0 => {
                self.out.write(&[val]);
                self.thr_ipending = true;
            },
            1 if self.dlab() => self.dlm = val,
            1 => {
                let v = val & 0x0f;
                if v & 0x02 != 0 && self.ier & 0x02 == 0 {
                    self.thr_ipending = true;
                }
                self.ier = v;
            },
            2 => self.fcr = val,
            3 => self.lcr = val,
            4 => self.mcr = val,
            7 => self.scr = val,
            _ => ()
        }
    }
}

impl MMIODevice for UART {
    fn handle(&mut self, mmio_reg: u64, mmio: &mut MMIO, _mem: &GuestMemory) -> Result<()> {
        if mmio.is_write != 0 {
            self.write(mmio_reg, mmio.data[0]);
        } else {
            mmio.data = [0; 8];
            mmio.data[0] = self.read(mmio_reg);
        }
        self.update_irq()
    }
}

impl VM {
    pub fn arch_init_mmio_devices(&mut self, config: &VmConfig) -> Result<()> {
        self.add_mmio_device(PLIC_MMIO_ADDR, PLIC_MMIO_SIZE, PLIC.clone() as Arc<Mutex<dyn MMIODevice>>);
        let uart = UART::new(self.fd, self.control.clone(), &config.serial)?;
        self.add_mmio_device(UART_MMIO_ADDR, UART_MMIO_SIZE, uart as Arc<Mutex<dyn MMIODevice>>);
        self.init_virtio_blk(config, VIRTIO_MMIO_BASE, VIRTIO_BLK_IRQ)?;
        for slot in 1..VIRTIO_MMIO_SLOTS {
            self.add_mmio_device(VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE, VIRTIO_MMIO_SIZE, VirtioPlaceholder::new());
        }
        Ok(())
    }
}
