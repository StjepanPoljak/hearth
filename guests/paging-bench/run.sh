#!/bin/bash
# Times, in a Linux guest, what shadow paging makes slow: bench.c, the
# guest's /init, forks and execs, takes page faults and switches between
# processes, then reboots. Each of RUNS boots (default 5) is under KVM's
# current paging mode; the table printed at the end has one row for each
# mode benchmarked so far, in microseconds per operation, mean ± standard
# deviation.
#
# Usage: guests/paging-bench/run.sh KERNEL          benchmark, then print
#        guests/paging-bench/run.sh report          only print the table
#        sudo guests/paging-bench/run.sh set EPT TDP_MMU
#                                                   reload KVM, e.g. set 0 1
#
# KERNEL is an x86_64 PVH vmlinux, e.g. from make-guest-kernel.sh. Needs
# gcc with a static libc, and /dev/kvm. HEARTH picks the binary
# [default: target/release/hearth]. Results go to build/paging-bench/.

set -euo pipefail

DIR=$(cd "$(dirname "$0")" && pwd)
ROOT=$DIR/../..
HEARTH=${HEARTH:-$ROOT/target/release/hearth}
RUNS=${RUNS:-5}
OUT=$ROOT/build/paging-bench
TESTS="fork_exec fork_cow page_fault ctx_switch"

yn01() { case "$1" in Y|1) echo 1 ;; *) echo 0 ;; esac; }

mode() {
    [ -d /sys/module/kvm_intel ] || { echo "(!) kvm_intel is not loaded." >&2; exit 1; }
    EPT=$(yn01 "$(cat /sys/module/kvm_intel/parameters/ept)")
    TDP=$(yn01 "$(cat /sys/module/kvm/parameters/tdp_mmu)")
    LABEL=ept$EPT-tdp$TDP
}

run() {
    local kernel=$1
    mode
    mkdir -p "$OUT"
    gcc -O2 -static -Wall -o "$OUT/init" "$DIR/bench.c"
    python3 -I "$ROOT/guests/nested/mkcpio.py" "$OUT/init.cpio" \
        dev/ dev/console:c:5:1 init="$OUT/init"

    local dir=$OUT/$LABEL
    rm -rf "$dir"
    mkdir -p "$dir"
    echo "run test ops us us_per_op" >"$dir/runs.txt"
    echo "$LABEL, $RUNS runs:"
    for i in $(seq 1 "$RUNS"); do
        local log=$dir/$i.log
        timeout 300 "$HEARTH" --linux "$kernel" --initramfs "$OUT/init.cpio" \
            --memory 512M --append "console=ttyS0 quiet" \
            --serial "file:$log" </dev/null >/dev/null 2>&1 || true
        grep -aq '^bench: done' "$log" || { echo "(!) No results in $log" >&2; exit 1; }
        # bench: fork_exec 2000 ops 782232 us 391.12 us/op
        grep -a '^bench: .* us/op' "$log" | awk -v i="$i" '{ print i, $2, $3, $5, $7 }' >>"$dir/runs.txt"
        echo "  #$i: $(awk -v i="$i" '$1 == i { printf "%s %s  ", $2, $5 }' "$dir/runs.txt")"
    done
    [ -n "${SUDO_USER:-}" ] && chown -R "$SUDO_USER:" "$OUT" || true
}

# "mean ± sd" of test $2's us/op in runs.txt $1.
us() {
    awk -v t="$2" 'NR > 1 && $2 == t { n++; s += $5; ss += $5 * $5 }
        END { if (!n) { print "–"; exit }
              m = s / n; v = n > 1 ? (ss - n * m * m) / (n - 1) : 0
              printf "%.2f ± %.2f", m, (v > 0 ? sqrt(v) : 0) }' "$1"
}

report() {
    local h="| Mode | runs |" s="|---|---|"
    for t in $TESTS; do h="$h $t |"; s="$s---:|"; done
    echo "$h"
    echo "$s"
    for label in ept0-tdp1 ept0-tdp0 ept1-tdp1 ept1-tdp0; do
        local f=$OUT/$label/runs.txt
        [ -f "$f" ] || continue
        # tdp_mmu doesn't apply with ept=0: show one ept0 row.
        [ "$label" = ept0-tdp0 ] && [ -f "$OUT/ept0-tdp1/runs.txt" ] && continue
        local name=$label n row
        [ "${label:3:1}" = 0 ] && name=ept0
        n=$(awk 'NR > 1 { r[$1] } END { print length(r) }' "$f")
        row="| $name | $n |"
        for t in $TESTS; do row="$row $(us "$f" "$t") |"; done
        echo "$row"
    done
}

set_mode() {
    [ $# -eq 2 ] || { echo "(!) Usage: sudo $0 set EPT TDP_MMU" >&2; exit 3; }
    modprobe -r kvm_intel kvm || { echo "(!) Could not unload KVM, are VMs running?" >&2; exit 2; }
    modprobe kvm tdp_mmu="$2"
    modprobe kvm_intel ept="$1"
    mode
    echo "KVM is now $LABEL."
}

case "${1:-}" in
    "")     echo "(!) Usage: $0 KERNEL | report | set EPT TDP_MMU" >&2; exit 3 ;;
    report) report ;;
    set)    shift; set_mode "$@" ;;
    *)      run "$1"; report ;;
esac
