# init for the innermost Linux on riscv64: print, then reboot, which
# ends Hearth. Like hello.asm.
        .section .rodata
msg:    .ascii "\nHello world from Linux inside Hearth inside QEMU (riscv64)!\n\n"
        .set len, . - msg               # before its use: li takes no forward symbols

        .globl _start
        .text
_start: li a7, 64                       # write(1, msg, len)
        li a0, 1
        la a1, msg
        li a2, len
        ecall
        li a7, 142                      # reboot(MAGIC1, MAGIC2, RESTART)
        li a0, 0xfee1dead
        li a1, 672274793
        li a2, 0x01234567
        ecall
1:      j 1b
