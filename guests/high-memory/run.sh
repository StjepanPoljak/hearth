#!/bin/sh
# Checks guest RAM above 4G: the guest goes to long mode with 5G of
# memory, writes the last qword below the 3G-4G hole, the first above it
# at 4G and the last at 6G, and must read each back; the hole and the
# end of RAM must not be RAM (Hearth reads 0 where there is no device).
# x86 only.
#
# Usage: guests/high-memory/run.sh   (needs nasm and /dev/kvm)
# HEARTH picks the binary [default: target/debug/hearth].

set -e
DIR=$(dirname "$0")
HEARTH=${HEARTH:-$DIR/../../target/debug/hearth}
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

nasm -f bin -o "$TMP/guest.bin" "$DIR/high-memory.asm"

# The guest idles at the end, so the VM still running at the timeout
# (status 124) is the pass; any other status means Hearth stopped it.
set +e
timeout 3 "$HEARTH" --binary "$TMP/guest.bin" --memory 5G \
    --serial "file:$TMP/serial.log" > "$TMP/out.txt" 2>&1
STATUS=$?
set -e

FAIL=0
check() {
    if grep -qs "$2" "$3"; then echo "ok:   $1"; else echo "FAIL: $1"; FAIL=1; fi
}
if [ $STATUS -eq 124 ]; then echo "ok:   VM kept running"; else echo "FAIL: VM stopped ($STATUS)"; FAIL=1; fi
check "RAM at 4G" "^4G=1111111111111111" "$TMP/serial.log"
check "RAM up to 6G" "^6G-8=2222222222222222" "$TMP/serial.log"
check "RAM up to 3G, apart from 4G" "^3G-8=3333333333333333" "$TMP/serial.log"
check "no RAM in the hole" "^hole=0000000000000000" "$TMP/serial.log"
check "no RAM past the end" "^end=0000000000000000" "$TMP/serial.log"
check "the guest finished" "^done" "$TMP/serial.log"
if [ $FAIL -ne 0 ]; then
    echo "--- Hearth:"; cat "$TMP/out.txt"; echo "--- serial:"; cat "$TMP/serial.log" 2>/dev/null
fi
exit $FAIL
