<p align="center">
  <img src="assets/hearth-4x.png" alt="Hearth" width="640">
</p>

Hearth is a small virtual machine monitor for Linux KVM, written in Rust
from scratch. It boots Linux on x86_64, aarch64 and riscv64, and on x86 it also runs
bare-metal Doom.

## Devices

| | x86_64 | aarch64 | riscv64 |
|---|---|---|---|
| Boot | PVH ELF (`vmlinux`), modules via `--initramfs` | `Image` with a device tree | `Image`; Hearth makes the device tree |
| Serial console | 16550 UART | UART | 16550 UART |
| Block | virtio-blk (MMIO) | virtio-blk (MMIO) | virtio-blk (MMIO) |
| Network | virtio-net (MMIO) on a tap | | |
| Keyboard | i8042 (PS/2) | | |
| Display | VGA mode 13h, served over VNC | | |

The guest gets one vcpu. On riscv64 Hearth emulates a PLIC for
interrupts, and the device layout is QEMU virt's. Hearth makes the
device tree there itself, with `--memory`, `--initramfs`, `--disk` and
`--append` (its bootargs) in it; `--dtb` gives one to use instead.

## Building

```sh
cargo build
```

For aarch64 or riscv64, build the cross-compilation image once, then build
inside it:

```sh
./docker-build.sh aarch64      # or riscv64
./docker-run.sh aarch64
```

## Running

Boot a Linux kernel with a root disk; the serial console is your terminal:

```sh
./make-rootfs.sh rootfs.img
target/debug/hearth --kernel vmlinux --disk rootfs.img
```

Disk settings go after the path, e.g.
`--disk rootfs.img,queue_size=256,seg_max=126` (see `--help`).

The same settings can live in a TOML file, given with `--config vm.toml`.
Each flag `--foo X` is `foo = X` there, and `--disk`'s settings go in a
`[disk]` table; flags on the command line override the file. Relative paths
are taken from the current directory, not the file's.

```toml
name = "web1"
kernel = "vmlinux"
memory = "1G"
append = "root=/dev/vda"
serial = "unix:/tmp/web1.sock"
tap = "tap0"

[disk]
path = "rootfs.img"
queue_size = 256
seg_max = 126
```

`--break` and `--dirty-log` are debugging aids and stay on the command line.

`--serial` connects the console elsewhere: `pty` (prints the device to open
with `screen`), `unix:PATH` (a socket for `socat - UNIX-CONNECT:PATH`; output
from before a client connects is kept, up to 64 KiB) or `file:PATH`.

Add networking with a tap device (`sudo ./make-tap.sh up`, then `--tap tap0`),
and a display and keyboard over VNC with `--vnc 127.0.0.1:5900`. VNC has no
authentication, so keep it on localhost or tunnel it.

`make-guest-kernel.sh` builds an x86_64 kernel with everything Hearth needs,
and `make-alpine-rootfs.sh` an Alpine rootfs for running containers.
`hearth --help` lists every option.

## Several VMs

Each VM is its own Hearth process. `run-vms.sh` starts several, each with
its own name, console socket, and optionally its own VNC port, tap and copy
of a disk image, all under `vms/`:

```sh
./run-vms.sh --disk rootfs.img --vnc 3 -- --kernel vmlinux
socat -,raw,echo=0,escape=0x1d UNIX-CONNECT:vms/vm1.sock   # Ctrl-] detaches
```

`--tap` attaches VM *i* to `tap`*i*, which `sudo ./make-tap.sh up tap`*i*
creates. Ctrl-C stops them all.

## Tests

```sh
cargo test
```

runs the unit tests. The test guests in `guests/` are small bare-metal
programs for x86 that check Hearth from inside a VM. Each one has a
`run.sh` that assembles it with nasm, boots it with `target/debug/hearth`
(or `HEARTH=...`), and prints an `ok:` or `FAIL:` line per check:

- `guests/virtio-needs-reset/run.sh`: the guest hands virtio-blk a
  descriptor outside its memory. Hearth must mark only that device
  DEVICE_NEEDS_RESET, raise a config interrupt, and keep the VM running.
  The guest's reset of the device must then clear it.
- `guests/high-memory/run.sh`: with `--memory 5G`, the guest enters long
  mode and writes RAM below the 3G–4G device hole, at 4G and up to 6G,
  then reads it back. The hole and the area past the end of RAM must not
  be RAM.

They need `/dev/kvm`, and so don't run in `cargo test`.

`guests/nested/run-riscv64-hearth.sh [--linux [--disk] | --binary] KERNEL`
tests riscv64 without RISC-V hardware: QEMU (TCG, with the hypervisor
extension) runs Hearth, which boots the same kernel. `--disk` also checks
virtio-blk and its interrupts. `--binary` boots a bare-metal hello world
instead of Linux. The script's header says how to build the kernel.

## Doom

`guests/doom` is doomgeneric with a small libc and a PVH entry stub, drawing
to VGA mode 13h and reading the PS/2 keyboard:

```sh
cargo build
make -C guests/doom run
```

then connect a VNC client to 127.0.0.1:5900 (`make run VNC=...` to change
it). The Makefile fetches doomgeneric and Freedoom into `build/`.

## Licence

Copyright © 2026 Stjepan Poljak.

Hearth is free software: you can redistribute it and/or modify it under
the terms of the GNU Affero General Public License as published by the Free
Software Foundation, either version 3 of the License, or (at your option)
any later version. See [LICENSE](LICENSE).

If you run a modified Hearth as a network service, the AGPL requires you
to offer its source code to the people who use it.

Companies that want to use Hearth without the AGPL's terms need a
commercial licence, agreed with the author and every contributor whose
code it covers. See [CONTRIBUTING.md](CONTRIBUTING.md).
