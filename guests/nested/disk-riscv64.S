# init for the innermost Linux on riscv64 with --disk: read sector 0 of
# /dev/vda, write sector 1 and read it back, all O_DIRECT so each goes
# through virtio-blk rather than the page cache. Prints what it reads,
# then /proc/interrupts, then reboots; run-riscv64.sh --disk checks it.
        .macro sys n
        li a7, \n
        ecall
        .endm
        .macro print str, len
        li a0, 1
        la a1, \str
        li a2, \len
        sys 64
        .endm
        .macro check want, failmsg, faillen     # a0 == want, else fail
        li t0, \want
        beq a0, t0, 9f
        print \failmsg, \faillen
        j dump
9:
        .endm
        .macro mount src, dir
        la a0, \src
        la a1, \dir
        la a2, \src
        li a3, 0
        li a4, 0
        sys 40
        .endm

        .equ AT_FDCWD, -100
        .equ O_RDWR_DIRECT, 0x4002
        .equ LINE, 62                   # the test lines' length, with \n

        .section .rodata
devtmpfs: .asciz "devtmpfs"
devdir: .asciz "/dev"
proc:   .asciz "proc"
procdir: .asciz "/proc"
vda:    .asciz "/dev/vda"
intr:   .asciz "/proc/interrupts"
m0:     .ascii "\ndisk: sector0="
        .set m0l, . - m0
m1:     .ascii "disk: sector1="
        .set m1l, . - m1
mdone:  .ascii "disk: done\n"
        .set mdonel, . - mdone
fo:     .ascii "disk: FAIL open /dev/vda\n"
        .set fol, . - fo
fr0:    .ascii "disk: FAIL read sector 0\n"
        .set fr0l, . - fr0
fw1:    .ascii "disk: FAIL write sector 1\n"
        .set fw1l, . - fw1
fr1:    .ascii "disk: FAIL read sector 1\n"
        .set fr1l, . - fr1

        .data
        .balign 4096                    # O_DIRECT wants aligned buffers
wbuf:   .ascii "written by the inner guest through virtio-blk, sector 1      \n"
        .space 512 - LINE
        .bss
        .balign 4096
rbuf:   .space 4096
ibuf:   .space 4096

        .globl _start
        .text
_start: mount devtmpfs, devdir
        mount proc, procdir
        li a0, AT_FDCWD
        la a1, vda
        li a2, O_RDWR_DIRECT
        sys 56                          # openat
        bgez a0, 1f
        print fo, fol
        j dump
1:      mv s0, a0
        mv a0, s0
        la a1, rbuf
        li a2, 512
        li a3, 0
        sys 67                          # pread64(fd, rbuf, 512, 0)
        check 512, fr0, fr0l
        print m0, m0l
        print rbuf, LINE
        mv a0, s0
        la a1, wbuf
        li a2, 512
        li a3, 512
        sys 68                          # pwrite64(fd, wbuf, 512, 512)
        check 512, fw1, fw1l
        mv a0, s0
        la a1, rbuf
        li a2, 512
        li a3, 512
        sys 67                          # pread64(fd, rbuf, 512, 512)
        check 512, fr1, fr1l
        print m1, m1l
        print rbuf, LINE

dump:   li a0, AT_FDCWD
        la a1, intr
        li a2, 0
        sys 56                          # openat(AT_FDCWD, intr, O_RDONLY)
        mv s1, a0
2:      mv a0, s1
        la a1, ibuf
        li a2, 4096
        sys 63                          # read
        blez a0, 3f
        mv a2, a0
        li a0, 1
        la a1, ibuf
        sys 64                          # write(1, ibuf, n)
        j 2b
3:      print mdone, mdonel
        li a0, 0xfee1dead               # reboot(MAGIC1, MAGIC2, RESTART)
        li a1, 672274793
        li a2, 0x01234567
        sys 142
4:      j 4b
