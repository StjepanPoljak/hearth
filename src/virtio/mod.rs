#[allow(non_camel_case_types, non_upper_case_globals, non_snake_case, dead_code)]
mod virtio_bindings {
    include!(concat!(env!("OUT_DIR"), "/virtio-blk-bindings.rs"));
    include!(concat!(env!("OUT_DIR"), "/virtio-ring-bindings.rs"));
}

/* Something the guest's driver got wrong: a malformed request or a
 * vring pointing outside guest memory. It breaks only the device it was
 * sent to, which then needs a reset; the VM keeps running. */
#[derive(Debug)]
pub struct GuestError(pub String);

impl std::fmt::Display for GuestError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

mod queue;
mod mmio;
mod blk;
mod net;

pub use queue::*;
pub use mmio::*;
pub use blk::*;
pub use net::*;

impl VM {
    /* Without a disk the slot still answers, as an empty one: the
     * guest is told about it either way. */
    pub fn init_virtio_blk(&mut self, config: &VmConfig, addr: u64, irq: u32) -> Result<()> {
        match &config.disk {
            Some(disk) => {
                let virtio_blk = VirtioMmio::new(VirtioBlk::new(disk)?, self.fd, irq);
                self.add_mmio_device(addr, VIRTIO_MMIO_SIZE, virtio_blk);
            }
            None => self.add_mmio_device(addr, VIRTIO_MMIO_SIZE, VirtioPlaceholder::new())
        }
        Ok(())
    }

    pub fn init_virtio_net(&mut self, config: &VmConfig, addr: u64, irq: u32) -> Result<()> {
        match &config.tap {
            Some(tap) => {
                let virtio_net = VirtioMmio::new(VirtioNet::new(tap, self.control.clone())?, self.fd, irq);
                self.add_mmio_device(addr, VIRTIO_MMIO_SIZE, virtio_net);
            }
            None => self.add_mmio_device(addr, VIRTIO_MMIO_SIZE, VirtioPlaceholder::new())
        }
        Ok(())
    }
}
