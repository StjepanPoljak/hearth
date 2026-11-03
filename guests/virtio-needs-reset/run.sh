#!/bin/sh
# Checks that a guest breaking virtio-blk breaks only the device: the
# guest points a descriptor outside its memory, and Hearth must mark the
# device DEVICE_NEEDS_RESET, send a config interrupt, keep the VM
# running, and clear it all when the guest resets the device. x86 only.
#
# Usage: guests/virtio-needs-reset/run.sh   (needs nasm and /dev/kvm)
# HEARTH picks the binary [default: target/debug/hearth].

set -e
DIR=$(dirname "$0")
HEARTH=${HEARTH:-$DIR/../../target/debug/hearth}
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

nasm -f bin -o "$TMP/guest.bin" "$DIR/virtio-needs-reset.asm"
truncate -s 1M "$TMP/disk.img"

# The guest idles at the end, so the VM still running at the timeout
# (status 124) is the pass; any other status means Hearth stopped it.
set +e
timeout 3 "$HEARTH" --binary "$TMP/guest.bin" --disk "$TMP/disk.img" \
    --serial "file:$TMP/serial.log" > "$TMP/out.txt" 2>&1
STATUS=$?
set -e

FAIL=0
check() {
    if grep -q "$2" "$3"; then echo "ok:   $1"; else echo "FAIL: $1"; FAIL=1; fi
}
if [ $STATUS -eq 124 ]; then echo "ok:   VM kept running"; else echo "FAIL: VM stopped ($STATUS)"; FAIL=1; fi
check "Hearth reported the guest's error" "virtio-blk: .*needs a reset" "$TMP/out.txt"
check "STATUS has NEEDS_RESET, config interrupt raised" "^S=4f I=02" "$TMP/serial.log"
check "a reset cleared both" "^S=00 I=00" "$TMP/serial.log"
if [ $(grep -c "needs a reset" "$TMP/out.txt") -ne 1 ]; then
    echo "FAIL: the broken device processed the second notify"; FAIL=1
else
    echo "ok:   the second notify was ignored"
fi
if [ $FAIL -ne 0 ]; then
    echo "--- Hearth:"; cat "$TMP/out.txt"; echo "--- serial:"; cat "$TMP/serial.log"
fi
exit $FAIL
