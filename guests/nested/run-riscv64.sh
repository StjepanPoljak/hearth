#!/bin/sh
# run.sh for riscv64, under TCG: QEMU's virt machine, with the hypervisor
# extension, boots KERNEL from an initramfs whose /init is a static
# riscv64 Hearth, which boots the same KERNEL, with a device tree Hearth
# makes, and an initramfs whose /init (hello-riscv64.S) prints a line and
# reboots.
# Hearth then exits, and the outer kernel's "Attempted to kill init" panic
# makes QEMU quit. Everything after "--" on QEMU's kernel command line is
# Hearth's argv; the kernel keeps a double-quoted value, spaces and all,
# in one argument.
#
# With --disk, Hearth gets a 1M disk whose sector 0 holds a line written
# here, and the inner /init is disk-riscv64.S instead: it reads sector 0,
# writes sector 1 and reads it back through virtio-blk, and prints
# /proc/interrupts. This then prints an ok: or FAIL: line per check, and
# exits 1 if any failed.
#
# Usage: guests/nested/run-riscv64.sh [--disk] KERNEL
#
# KERNEL is a riscv64 Image with KVM built in, e.g. from a clean tree:
#   docker run --rm -v "$PWD":/w -w /w riscv64-hearth-build sh -c '
#     S=build/linux-src; O=build/riscv-linux
#     M="make -C $S O=$PWD/$O ARCH=riscv CROSS_COMPILE=riscv64-linux-gnu-"
#     $M defconfig && echo CONFIG_KVM=y > $O/kvm.config &&
#     $S/scripts/kconfig/merge_config.sh -m -O $O $O/.config $O/kvm.config &&
#     $M olddefconfig && $M -j$(nproc) Image'
# which gives build/riscv-linux/arch/riscv/boot/Image. Needs the
# riscv64-hearth-build image (./docker-build.sh riscv64) and
# qemu-system-riscv64. Build output goes to build/nested-riscv64.

set -e
DISK=
if [ "$1" = --disk ]; then
    DISK=1
    shift
fi
if [ -z "$1" ]; then
    echo "(!) Usage: $0 [--disk] KERNEL" >&2
    exit 1
fi
KERNEL=$(realpath "$1")
DIR=$(realpath "$(dirname "$0")")
ROOT=$(realpath "$DIR/../..")
OUT=$ROOT/build/nested-riscv64
mkdir -p "$OUT"

if [ -n "$DISK" ]; then INIT=disk; else INIT=hello; fi

# Static Hearth and the inner init, built in the cross image as
# docker-run.sh does it.
docker run --rm -v "$ROOT":/home/docker/hearth -w /home/docker/hearth \
    -e RUSTFLAGS="-C target-feature=+crt-static -C relocation-model=static" \
    -e CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=riscv64-linux-gnu-gcc \
    riscv64-hearth-build sh -c "
        set -e
        cargo build --release --target=riscv64gc-unknown-linux-gnu
        riscv64-linux-gnu-as -o build/nested-riscv64/$INIT.o guests/nested/$INIT-riscv64.S
        riscv64-linux-gnu-ld -static -o build/nested-riscv64/$INIT build/nested-riscv64/$INIT.o"
HEARTH=$ROOT/target/riscv64gc-unknown-linux-gnu/release/hearth

python3 -I "$DIR/mkcpio.py" "$OUT/inner.cpio" \
    dev/ dev/console:c:5:1 proc/ init="$OUT/$INIT"

HEARTH_ARGS="--linux /Image --initramfs /inner.cpio --memory 256M"
HEARTH_ARGS="$HEARTH_ARGS --append=\"console=ttyS0 earlycon panic=-1\""
DISK_FILES=
if [ -n "$DISK" ]; then
    # A 62-byte line (with its \n), as disk-riscv64.S prints sector 0.
    python3 -I -c 'import sys
line = b"HEARTH TEST DISK, sector 0, written on the host".ljust(61) + b"\n"
open(sys.argv[1], "wb").write(line + bytes((1 << 20) - len(line)))' "$OUT/disk.img"
    HEARTH_ARGS="$HEARTH_ARGS --disk /disk.img"
    DISK_FILES="disk.img=$OUT/disk.img"
fi

python3 -I "$DIR/mkcpio.py" "$OUT/outer.cpio" \
    dev/ dev/console:c:5:1 dev/kvm:c:10:232 \
    init="$HEARTH" Image="$KERNEL" \
    inner.cpio="$OUT/inner.cpio" $DISK_FILES

run_qemu() {
    qemu-system-riscv64 -machine virt -cpu rv64,h=true -m 1G -nographic -no-reboot \
        -kernel "$KERNEL" -initrd "$OUT/outer.cpio" \
        -append "console=ttyS0 quiet panic=-1 -- $HEARTH_ARGS" < /dev/null
}

if [ -z "$DISK" ]; then
    run_qemu
    exit
fi

LOG=$OUT/disk.log
run_qemu > "$LOG" 2>&1 || true

FAIL=0
check() {
    if grep -qs "$2" "$LOG"; then echo "ok:   $1"; else echo "FAIL: $1"; FAIL=1; fi
}
check "the guest finds a 1M virtio disk" "virtio_blk virtio0: \[vda\] 2048 512-byte logical blocks"
check "sector 0 has what was written on the host" "^disk: sector0=HEARTH TEST DISK, sector 0, written on the host"
check "sector 1 reads back what the guest wrote" "^disk: sector1=written by the inner guest through virtio-blk, sector 1"
check "virtio-blk interrupts arrive through the PLIC" "^ *[0-9]*: *[1-9][0-9]* *SiFive PLIC *1 .*virtio0"
check "the guest finished" "^disk: done"
if [ $FAIL -ne 0 ]; then
    echo "--- QEMU, Hearth and the guests (also in $LOG):"; cat "$LOG"
fi
exit $FAIL
