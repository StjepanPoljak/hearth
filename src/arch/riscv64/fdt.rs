/* The device tree Linux boots with on riscv64, made at boot: RAM, the
 * command line and the initramfs from the VM's configuration, and the
 * devices of mmio.rs and plic.rs. */

use anyhow::{ anyhow, Result };
use vm_fdt::FdtWriter;

use super::mmio::{ UART_MMIO_ADDR, UART_MMIO_SIZE, UART_IRQ };
use super::mmio::{ VIRTIO_MMIO_BASE, VIRTIO_MMIO_STRIDE, VIRTIO_BLK_IRQ };
use super::plic::{ PLIC_MMIO_ADDR, PLIC_MMIO_SIZE, PLIC_SOURCES };

/* The guest reads the host's time CSR: QEMU virt's 10 MHz. */
const TIMEBASE_FREQUENCY: u32 = 10_000_000;
/* What KVM gives a guest by default. riscv,isa is for kernels older than
 * riscv,isa-base and riscv,isa-extensions. */
const ISA: &str = "rv64imafdc_zicsr_zifencei";
const ISA_EXTENSIONS: [&str; 8] = ["i", "m", "a", "f", "d", "c", "zicsr", "zifencei"];
/* The S-mode external interrupt, the PLIC's context 0. */
const IRQ_S_EXT: u32 = 9;

const CPU_INTC_PHANDLE: u32 = 1;
const PLIC_PHANDLE: u32 = 2;

pub struct FdtParams<'a> {
    pub mem_start: u64,
    pub mem_size: u64,
    pub bootargs: &'a str,
    /* Start and end, the end exclusive. */
    pub initrd: Option<(u64, u64)>,
    pub virtio_blk: bool
}

pub fn create_fdt(params: &FdtParams) -> Result<Vec<u8>> {
    build(params).map_err(|e| anyhow!("Failed to build the device tree: {}", e))
}

fn build(params: &FdtParams) -> Result<Vec<u8>, vm_fdt::Error> {
    let mut fdt = FdtWriter::new()?;
    let root = fdt.begin_node("")?;
    fdt.property_u32("#address-cells", 2)?;
    fdt.property_u32("#size-cells", 2)?;
    fdt.property_string("compatible", "riscv-virtio")?;
    fdt.property_string("model", "hearth")?;

    let uart_node = format!("/soc/serial@{:x}", UART_MMIO_ADDR);
    let chosen = fdt.begin_node("chosen")?;
    fdt.property_string("bootargs", params.bootargs)?;
    fdt.property_string("stdout-path", &uart_node)?;
    if let Some((start, end)) = params.initrd {
        fdt.property_u64("linux,initrd-start", start)?;
        fdt.property_u64("linux,initrd-end", end)?;
    }
    fdt.end_node(chosen)?;

    let memory = fdt.begin_node(&format!("memory@{:x}", params.mem_start))?;
    fdt.property_string("device_type", "memory")?;
    fdt.property_array_u64("reg", &[params.mem_start, params.mem_size])?;
    fdt.end_node(memory)?;

    let cpus = fdt.begin_node("cpus")?;
    fdt.property_u32("#address-cells", 1)?;
    fdt.property_u32("#size-cells", 0)?;
    fdt.property_u32("timebase-frequency", TIMEBASE_FREQUENCY)?;
    let cpu = fdt.begin_node("cpu@0")?;
    fdt.property_string("device_type", "cpu")?;
    fdt.property_u32("reg", 0)?;
    fdt.property_string("compatible", "riscv")?;
    fdt.property_string("riscv,isa", ISA)?;
    fdt.property_string("riscv,isa-base", "rv64i")?;
    fdt.property_string_list("riscv,isa-extensions",
                             ISA_EXTENSIONS.iter().map(|s| s.to_string()).collect())?;
    fdt.property_string("mmu-type", "riscv,sv39")?;
    fdt.property_string("status", "okay")?;
    let intc = fdt.begin_node("interrupt-controller")?;
    fdt.property_string("compatible", "riscv,cpu-intc")?;
    fdt.property_u32("#interrupt-cells", 1)?;
    fdt.property_null("interrupt-controller")?;
    fdt.property_phandle(CPU_INTC_PHANDLE)?;
    fdt.end_node(intc)?;
    fdt.end_node(cpu)?;
    fdt.end_node(cpus)?;

    let soc = fdt.begin_node("soc")?;
    fdt.property_string("compatible", "simple-bus")?;
    fdt.property_u32("#address-cells", 2)?;
    fdt.property_u32("#size-cells", 2)?;
    fdt.property_null("ranges")?;

    let plic = fdt.begin_node(&format!("interrupt-controller@{:x}", PLIC_MMIO_ADDR))?;
    fdt.property_string_list("compatible",
                             vec!["sifive,plic-1.0.0".to_string(), "riscv,plic0".to_string()])?;
    fdt.property_array_u64("reg", &[PLIC_MMIO_ADDR, PLIC_MMIO_SIZE])?;
    fdt.property_u32("#address-cells", 0)?;
    fdt.property_u32("#interrupt-cells", 1)?;
    fdt.property_null("interrupt-controller")?;
    fdt.property_array_u32("interrupts-extended", &[CPU_INTC_PHANDLE, IRQ_S_EXT])?;
    /* Source 0 means "no interrupt". */
    fdt.property_u32("riscv,ndev", PLIC_SOURCES as u32 - 1)?;
    fdt.property_phandle(PLIC_PHANDLE)?;
    fdt.end_node(plic)?;

    let serial = fdt.begin_node(&format!("serial@{:x}", UART_MMIO_ADDR))?;
    fdt.property_string("compatible", "ns16550a")?;
    fdt.property_array_u64("reg", &[UART_MMIO_ADDR, UART_MMIO_SIZE])?;
    fdt.property_u32("clock-frequency", 3_686_400)?;
    fdt.property_u32("interrupt-parent", PLIC_PHANDLE)?;
    fdt.property_u32("interrupts", UART_IRQ)?;
    fdt.end_node(serial)?;

    if params.virtio_blk {
        let virtio = fdt.begin_node(&format!("virtio_mmio@{:x}", VIRTIO_MMIO_BASE))?;
        fdt.property_string("compatible", "virtio,mmio")?;
        fdt.property_array_u64("reg", &[VIRTIO_MMIO_BASE, VIRTIO_MMIO_STRIDE])?;
        fdt.property_u32("interrupt-parent", PLIC_PHANDLE)?;
        fdt.property_u32("interrupts", VIRTIO_BLK_IRQ)?;
        fdt.end_node(virtio)?;
    }

    fdt.end_node(soc)?;
    fdt.end_node(root)?;
    fdt.finish()
}
