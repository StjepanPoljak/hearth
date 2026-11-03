; Breaks virtio-blk's queue 0 with a descriptor outside guest memory,
; then prints STATUS and INTERRUPT_STATUS, resets the device and prints
; them again: "S=xx I=xx" lines on COM1.
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
        mov ebx, 0xd0000000            ; virtio-blk MMIO

        mov dword [ebx+0x70], 0        ; STATUS: reset
        mov dword [ebx+0x70], 0xb      ; ACK | DRIVER | FEATURES_OK
        mov dword [ebx+0x30], 0        ; QUEUE_SEL
        mov dword [ebx+0x38], 4        ; QUEUE_NUM
        mov dword [ebx+0x80], 0x10000  ; QUEUE_DESC_LOW
        mov dword [ebx+0x90], 0x11000  ; QUEUE_AVAIL_LOW
        mov dword [ebx+0xa0], 0x12000  ; QUEUE_USED_LOW
        mov dword [ebx+0x44], 1        ; QUEUE_READY
        mov dword [ebx+0x70], 0xf      ; | DRIVER_OK

        mov dword [0x10000], 0         ; desc 0: addr 4G, outside memory
        mov dword [0x10004], 1
        mov dword [0x10008], 16
        mov dword [0x1000c], 0
        mov word [0x11000], 0          ; avail: flags, idx 1, ring[0] = 0
        mov word [0x11004], 0
        mov word [0x11002], 1
        mov dword [ebx+0x50], 0        ; QUEUE_NOTIFY
        mov dword [ebx+0x50], 0        ; again: must be ignored
        call report

        mov dword [ebx+0x70], 0        ; reset clears NEEDS_RESET
        call report
.idle:  jmp .idle

report: mov al, 'S'
        call putc
        mov al, '='
        call putc
        mov eax, [ebx+0x70]
        call hex
        mov al, ' '
        call putc
        mov al, 'I'
        call putc
        mov al, '='
        call putc
        mov eax, [ebx+0x60]
        call hex
        mov al, 10
        call putc
        ret

hex:    push eax
        shr al, 4
        call nibble
        pop eax
nibble: and al, 0xf
        add al, '0'
        cmp al, '9'
        jbe putc
        add al, 'a' - '9' - 1
putc:   mov dx, 0x3f8
        out dx, al
        ret

gdt:    dq 0
        dq 0x00cf9a000000ffff          ; code
        dq 0x00cf92000000ffff          ; data
gdt_desc:
        dw gdt_desc - gdt - 1
        dd gdt
