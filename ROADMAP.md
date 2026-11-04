# Roadmap

## Device I/O without vcpu kicks

Today the virtio-net RX thread can't touch guest memory, so it kicks a vcpu
thread out of `KVM_RUN` with `SIGRTMIN` (`VmControl::kick()` in `src/main.rs`)
and that vcpu thread fills the RX buffers in `MMIODevice::poll()`.

Goal: device threads do their own I/O and never interrupt a vcpu.

- Give device threads a handle to guest memory (e.g. `Arc<GuestMemory>`).
  Guest memory is already complete before MMIO devices are created, and
  `MemRegion` is `Send + Sync`.
- Let the RX thread lock the `VirtioMmio`, fill the RX ring directly and raise
  the interrupt with `KVM_IRQ_LINE` (callable from any thread), or via an irqfd.
- Remove `VmControl::kick()`, `devices_pending` and `MMIODevice::poll()` once
  nothing uses them. The kick signal stays for `VmControl::stop()`.

## Untested paths

- **Linux on x86 since the MMIO and memory changes** (`6f5990d`, `cd2b2b2`):
  the x86 test guests and Linux on aarch64 run that code, but no x86 Linux
  boot has yet. Check a `vmlinux` boot, with and without `--disk`/`--tap`,
  and with `--memory` above 3G.
- **aarch64 guest power off and reset:** Hearth handles
  `KVM_SYSTEM_EVENT_SHUTDOWN`/`RESET` (PSCI), but the `virt` DTB on the Pi has
  no PSCI node, so the guest never asks. Needs a DTB with
  `psci { compatible = "arm,psci-1.0"; method = "hvc"; }` (and the vcpu set up
  for PSCI), then `poweroff` and `reboot` in the guest.

## aarch64 --binary memory

`--binary` RAM starts at 0 and must end below the GIC at 0x8000000, so it is
limited to 128M (more is an error). For more, move binary RAM above the
devices, e.g. to 1G where Linux's RAM is, with the load address following it.
