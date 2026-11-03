; Goes to long mode, writes RAM on both sides of the hole below 4G and
; reads it back, then prints "NAME=VALUE" lines (64-bit hex) on COM1.
; Run with --memory 5G: RAM is 0-3G and 4G-6G, the hole 3G-4G.
bits 16
org 0x1000
        cli
        lgdt [gdt_desc]
        mov eax, cr0
        or eax, 1
        mov cr0, eax
        jmp 0x08:pm

bits 32
pm:     mov ax, 0x10
        mov ds, ax
        mov es, ax
        mov ss, ax
        mov esp, 0x30000

        ; Identity map 0-8G with 2M pages: PML4 at 0x20000, PDPT at
        ; 0x21000, eight page directories from 0x22000.
        cld
        mov edi, 0x20000
        mov ecx, 10 * 1024             ; 10 pages of dwords
        xor eax, eax
        rep stosd
        mov dword [0x20000], 0x21003   ; PML4[0] -> PDPT, present + rw
        mov edi, 0x21000
        mov eax, 0x22003
        mov ecx, 8
.pdpt:  mov [edi], eax                 ; PDPT[i] -> PD i
        add eax, 0x1000
        add edi, 8
        loop .pdpt
        mov edi, 0x22000
        xor eax, eax                   ; physical address, low dword
        xor edx, edx                   ; high dword
        mov ecx, 8 * 512
.pd:    mov ebx, eax
        or ebx, 0x83                   ; present + rw + 2M page
        mov [edi], ebx
        mov [edi+4], edx
        add edi, 8
        add eax, 0x200000
        adc edx, 0
        loop .pd

        mov eax, cr4
        or eax, 1 << 5                 ; PAE
        mov cr4, eax
        mov eax, 0x20000
        mov cr3, eax
        mov ecx, 0xc0000080            ; EFER
        rdmsr
        or eax, 1 << 8                 ; LME
        wrmsr
        mov eax, cr0
        or eax, 1 << 31                ; PG
        mov cr0, eax
        jmp 0x18:lm

bits 64
default abs                            ; [qword N] is an address, not rip-relative
lm:     mov rax, 0x1111111111111111
        mov [qword 0x100000000], rax   ; first qword above the hole
        mov rax, 0x2222222222222222
        mov [qword 0x17ffffff8], rax   ; last qword of RAM, at 6G
        mov rax, 0x3333333333333333
        mov [qword 0xbffffff8], rax    ; last qword below the hole
        mov rax, 0x4444444444444444
        mov [qword 0xc0000000], rax    ; the hole: not RAM, reads 0
        mov [qword 0x180000000], rax   ; past the end: not RAM either

        mov rsi, name_4g
        mov rax, [qword 0x100000000]
        call report
        mov rsi, name_6g
        mov rax, [qword 0x17ffffff8]
        call report
        mov rsi, name_3g
        mov rax, [qword 0xbffffff8]
        call report
        mov rsi, name_hole
        mov rax, [qword 0xc0000000]
        call report
        mov rsi, name_end
        mov rax, [qword 0x180000000]
        call report
        mov rsi, name_done
        call puts
.idle:  jmp .idle

; Prints the string at rsi, '=', rax in hex and a newline.
report: mov rbx, rax
        call puts
        mov al, '='
        call putc
        mov ecx, 16
.digit: rol rbx, 4
        mov al, bl
        and al, 0xf
        add al, '0'
        cmp al, '9'
        jbe .out
        add al, 'a' - '9' - 1
.out:   call putc
        loop .digit
        mov al, 10
        jmp putc

puts:   lodsb
        test al, al
        jz .end
        call putc
        jmp puts
.end:   ret

putc:   mov dx, 0x3f8
        out dx, al
        ret

name_4g:   db "4G", 0
name_6g:   db "6G-8", 0
name_3g:   db "3G-8", 0
name_hole: db "hole", 0
name_end:  db "end", 0
name_done: db "done", 10, 0

gdt:    dq 0
        dq 0x00cf9a000000ffff          ; 0x08: 32-bit code
        dq 0x00cf92000000ffff          ; 0x10: data
        dq 0x00af9a000000ffff          ; 0x18: 64-bit code
gdt_desc:
        dw gdt_desc - gdt - 1
        dd gdt
