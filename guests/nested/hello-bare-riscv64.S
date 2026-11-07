# A bare-metal guest for Hearth's --binary on riscv64, no Linux: it
# writes a line to the ns16550a at 0x10000000, then asks SBI for a
# shutdown, which KVM turns into KVM_SYSTEM_EVENT_SHUTDOWN and Hearth
# exits. It runs in VS-mode from --load-addr (0x1000), with RAM from 0;
# la is pc-relative, so the address doesn't matter.
        .equ UART, 0x10000000
        .equ LSR, 5                     # line status; bit 5: THR empty

        .globl _start
        .text
_start: li t0, UART
        la t1, msg
1:      lbu t2, 0(t1)
        beqz t2, 3f
2:      lbu t3, LSR(t0)
        andi t3, t3, 0x20
        beqz t3, 2b
        sb t2, 0(t0)
        addi t1, t1, 1
        j 1b
3:      li a7, 0x53525354               # SBI SRST: system_reset(SHUTDOWN, NO_REASON)
        li a6, 0
        li a0, 0
        li a1, 0
        ecall
4:      wfi                             # if SRST isn't there
        j 4b

msg:    .asciz "\nHello world from bare metal inside Hearth inside QEMU (riscv64)!\n\n"
