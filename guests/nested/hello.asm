; init for the innermost Linux: print, then reboot, which ends Hearth.
bits 64
global _start
section .text
_start: mov eax, 1                     ; write(1, msg, len)
        mov edi, 1
        lea rsi, [rel msg]
        mov edx, len
        syscall
        mov eax, 169                   ; reboot(MAGIC1, MAGIC2, RESTART)
        mov edi, 0xfee1dead
        mov esi, 672274793
        mov edx, 0x01234567
        syscall
.hang:  jmp .hang
section .rodata
msg:    db 10, "Hello world from Linux inside Hearth inside QEMU!", 10, 10
len     equ $ - msg
