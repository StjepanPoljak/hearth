#!/bin/sh
#
# Start several Hearth VMs, one process each, giving every VM its own
# name, serial socket, VNC port, tap device and disk.
#
# Usage: ./run-vms.sh [options] COUNT [-- hearth options...]
#
#   --disk IMG   give each VM its own copy of IMG (made once, then kept)
#   --tap        attach VM i to tap<i> (sudo ./make-tap.sh up tap<i> ...)
#   --vnc        serve VM i's display on 127.0.0.1:<5900 + i>
#   --dir DIR    where the sockets, logs and disks go [default: vms]
#
# Everything after -- goes to every VM, e.g. --kernel vmlinux. VM i is
# named vm<i>; attach to its console with
#   socat -,raw,echo=0,escape=0x1d UNIX-CONNECT:vms/vm<i>.sock
# (Ctrl-] detaches). Ctrl-C stops them all.

set -e

HEARTH=${HEARTH:-$(dirname "$0")/target/debug/hearth}
DIR=vms
DISK=
TAP=
VNC=

usage() {
    sed -n '3,19p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --disk) DISK=$2; shift 2 ;;
        --tap)  TAP=1; shift ;;
        --vnc)  VNC=1; shift ;;
        --dir)  DIR=$2; shift 2 ;;
        -h|--help) usage ;;
        -*) echo "(!) Unknown option: $1" >&2; usage ;;
        *)  COUNT=$1; shift; break ;;
    esac
done
[ "$1" = "--" ] && shift

case "$COUNT" in
    ''|*[!0-9]*) usage ;;
esac
if [ ! -x "$HEARTH" ]; then
    echo "(!) No $HEARTH: run cargo build, or set HEARTH." >&2
    exit 1
fi
if [ -n "$DISK" ] && [ ! -f "$DISK" ]; then
    echo "(!) No disk image $DISK." >&2
    exit 1
fi
mkdir -p "$DIR"

# Check every VM's resources before starting any.
i=0
while [ $i -lt "$COUNT" ]; do
    sock=$DIR/vm$i.sock
    if [ ${#sock} -gt 107 ]; then
        echo "(!) $sock is too long for a Unix socket: use a shorter --dir." >&2
        exit 1
    fi
    if [ -n "$TAP" ] && ! ip link show "tap$i" >/dev/null 2>&1; then
        echo "(!) No tap$i: sudo ./make-tap.sh up tap$i 192.168.$((100 + i)).1/24" >&2
        exit 1
    fi
    i=$((i + 1))
done

PIDS=
stop() {
    trap - INT TERM
    [ -n "$PIDS" ] && kill $PIDS 2>/dev/null
    wait
    exit 0
}
trap stop INT TERM

# Starts VM $1 with the shared options that follow; a function, so the
# per-VM options it adds stay in its own "$@".
start() {
    vm=vm$1
    vnc=127.0.0.1:$((5900 + $1))
    tap=tap$1
    shift
    set -- "$@" --name "$vm" --serial "unix:$DIR/$vm.sock"
    [ -n "$VNC" ] && set -- "$@" --vnc "$vnc"
    [ -n "$TAP" ] && set -- "$@" --tap "$tap"
    if [ -n "$DISK" ]; then
        # Copy-on-write where the filesystem can, sparse where it can't.
        [ -f "$DIR/$vm.img" ] || cp --reflink=auto --sparse=always "$DISK" "$DIR/$vm.img"
        set -- "$@" --disk "$DIR/$vm.img"
    fi

    # A socket left by an earlier run would take connections until
    # Hearth replaces it; this way one that exists is live, or about to be.
    rm -f "$DIR/$vm.sock"
    "$HEARTH" "$@" </dev/null >"$DIR/$vm.log" 2>&1 &
    PIDS="$PIDS $!"
    echo "$vm: pid $!, console $DIR/$vm.sock, log $DIR/$vm.log$([ -n "$VNC" ] && echo ", VNC $vnc")"
}

i=0
while [ $i -lt "$COUNT" ]; do
    start $i "$@"
    i=$((i + 1))
done

echo "Attach: socat -,raw,echo=0,escape=0x1d UNIX-CONNECT:$DIR/vm0.sock (Ctrl-] detaches). Ctrl-C stops all."
wait
