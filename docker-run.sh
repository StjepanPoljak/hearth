#!/bin/sh

if [ -z ${1} ]; then
    echo "(!) Specify architecture: aarch64, riscv64."
    exit 1
fi

ARCH=${1}

if [ ${ARCH} != aarch64 ] && [ ${ARCH} != riscv64 ]; then
    echo "(!) Unknown architecture: ${ARCH}"
    exit 2
fi

TARGET=${ARCH}-unknown-linux-gnu
# Cargo links with the image's cc otherwise, which is the x86 one.
LINKER_VAR=CARGO_TARGET_$(echo ${TARGET} | tr 'a-z-' 'A-Z_')_LINKER

docker run --network host -it --rm \
       -v "$(pwd)":"/home/docker/hearth" -w /home/docker/hearth \
       -e RUSTFLAGS="-C target-feature=+crt-static" \
       -e ${LINKER_VAR}=${ARCH}-linux-gnu-gcc \
       ${ARCH}-hearth-build \
       cargo build --release --target=${TARGET}
