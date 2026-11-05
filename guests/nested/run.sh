#!/bin/sh
# Linux inside Hearth inside QEMU, with nothing else: QEMU boots KERNEL
# from an initramfs whose /init is a static Hearth, which boots the same
# KERNEL with an initramfs whose /init (hello.asm) prints a line and
# reboots. Hearth then exits, and the outer kernel's "Attempted to kill
# init" panic makes QEMU quit. Everything after "--" on QEMU's kernel
# command line is Hearth's argv. x86 only.
#
# Usage: guests/nested/run.sh KERNEL
#
# KERNEL is a PVH vmlinux with KVM built in (a module would need loading,
# and there is nothing to load it). The guest kernel with KVM added:
#   ./make-guest-kernel.sh ~/linux        # builds build/linux/vmlinux, then
#   printf 'CONFIG_KVM=y\nCONFIG_KVM_INTEL=y\n' > build/kvm.config
#   ~/linux/scripts/kconfig/merge_config.sh -m -O build/linux \
#       build/linux/.config build/kvm.config
#   make -C ~/linux O=$PWD/build/linux olddefconfig vmlinux
# (CONFIG_KVM_AMD=y on AMD.) The host needs nested KVM, QEMU and nasm.
# Build output goes to build/nested.

set -e
if [ -z "$1" ]; then
    echo "(!) Usage: $0 KERNEL" >&2
    exit 1
fi
KERNEL=$(realpath "$1")
DIR=$(realpath "$(dirname "$0")")
ROOT=$(realpath "$DIR/../..")
OUT=$ROOT/build/nested
mkdir -p "$OUT"

# Static, so it runs with no libraries in the initramfs. Its own target
# triple keeps it apart from the normal target/release build.
(cd "$ROOT" && RUSTFLAGS="-C target-feature=+crt-static" \
    cargo build -q --release --target x86_64-unknown-linux-gnu)
HEARTH=$ROOT/target/x86_64-unknown-linux-gnu/release/hearth

nasm -f elf64 -o "$OUT/hello.o" "$DIR/hello.asm"
ld -static -o "$OUT/hello" "$OUT/hello.o"
python3 -I "$DIR/mkcpio.py" "$OUT/hello.cpio" \
    dev/ dev/console:c:5:1 init="$OUT/hello"
python3 -I "$DIR/mkcpio.py" "$OUT/outer.cpio" \
    dev/ dev/console:c:5:1 dev/kvm:c:10:232 \
    init="$HEARTH" vmlinux="$KERNEL" hello.cpio="$OUT/hello.cpio"

exec qemu-system-x86_64 -enable-kvm -cpu host -m 1G -nographic -no-reboot \
    -kernel "$KERNEL" -initrd "$OUT/outer.cpio" \
    -append "console=ttyS0 quiet panic=-1 -- --linux /vmlinux --initramfs /hello.cpio --memory 256M"
