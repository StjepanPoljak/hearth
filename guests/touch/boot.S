/*
 * A guest that writes to one page and stops, to trace what KVM does for
 * a single fault, with EPT and with shadow paging.
 *
 * The VMM's PVH loader jumps to pvh_start in 32-bit protected mode with
 * flat segments and paging off. Build page tables that identity-map the
 * first 2 MiB with 4 KiB pages (so a guest walk has all four levels) and
 * map one more page, TOUCH_VA, to TOUCH_PA; enter long mode; write to
 * TOUCH_VA once; reset through the i8042, which ends the VMM.
 *
 * TOUCH_PA is outside the identity map and past the image, so neither the
 * VMM nor this code touches it before that write: its first access is
 * the write, and it misses all the way to a host page fault.
 *
 * Built with PAGING=0, it writes to TOUCH_PA straight from the entry,
 * paging still off: the address it uses is the GPA, and with shadow
 * paging KVM has no guest page tables to walk.
 */

#ifndef PAGING
#define PAGING		1
#endif

#define CR0_PE		(1 << 0)
#define CR0_PG		(1 << 31)
#define CR4_PAE		(1 << 5)
#define MSR_EFER	0xc0000080
#define EFER_LME	(1 << 8)
#define PTE_P		(1 << 0)
#define PTE_W		(1 << 1)
#define TOUCH_VA	0x100000000
#define TOUCH_PA	0x200000
#define I8042_COMMAND	0x64
#define I8042_RESET	0xfe		/* pulse the CPU reset line */
#define GDT_CODE	0x08
#define GDT_DATA	0x10

/* The VMM finds pvh_start through this note, in a section named .notes. */
	.section .notes, "a"
	.align 4
	.long	4			/* name size, "Xen\0" */
	.long	4			/* descriptor size */
	.long	18			/* XEN_ELFNOTE_PHYS32_ENTRY */
	.asciz	"Xen"
	.long	pvh_start

	.section .text.boot, "ax"
	.code32
	.global	pvh_start
pvh_start:
	cli
	cld

	/* Clear .bss: the page tables live there. */
	mov	$__bss_start, %edi
	mov	$__bss_end, %ecx
	sub	%edi, %ecx
	xor	%eax, %eax
	rep stosb

#if PAGING
	/* PML4[0] -> pdpt, pdpt[0] -> pd, pd[0] -> pt. */
	movl	$(pdpt + (PTE_P | PTE_W)), pml4
	movl	$(pd + (PTE_P | PTE_W)), pdpt
	movl	$(pt + (PTE_P | PTE_W)), pd

	/* pt: 512 entries of 4 KiB, physical address == virtual address. */
	mov	$(PTE_P | PTE_W), %eax
	xor	%ecx, %ecx
1:	mov	%eax, pt(, %ecx, 8)
	add	$0x1000, %eax
	inc	%ecx
	cmp	$512, %ecx
	jne	1b

	/* pdpt[4] -> pd_touch, pd_touch[0] -> pt_touch, pt_touch[0] -> TOUCH_PA. */
	movl	$(pd_touch + (PTE_P | PTE_W)), pdpt + ((TOUCH_VA >> 30) & 511) * 8
	movl	$(pt_touch + (PTE_P | PTE_W)), pd_touch + ((TOUCH_VA >> 21) & 511) * 8
	movl	$(TOUCH_PA + (PTE_P | PTE_W)), pt_touch + ((TOUCH_VA >> 12) & 511) * 8

	mov	%cr4, %eax
	or	$CR4_PAE, %eax
	mov	%eax, %cr4
	mov	$pml4, %eax
	mov	%eax, %cr3

	mov	$MSR_EFER, %ecx
	rdmsr
	or	$EFER_LME, %eax
	wrmsr

	/* Paging on with EFER.LME set activates long mode. */
	mov	%cr0, %eax
	or	$(CR0_PG | CR0_PE), %eax
	mov	%eax, %cr0

	lgdt	gdt_ptr
	ljmp	$GDT_CODE, $long_mode

	.code64
long_mode:
	mov	$GDT_DATA, %ax
	mov	%ax, %ds
	mov	%ax, %es
	mov	%ax, %ss

	/* The one access this guest exists for. */
	movabs	$TOUCH_VA, %rax
	movl	$0x1, (%rax)
#else
	/* The one access this guest exists for, to the GPA itself. */
	movl	$0x1, TOUCH_PA
#endif

	/* End the VM: the PC's reset through the i8042, which the VMM
	 * takes as a stop. (hlt would only block: KVM's in-kernel LAPIC
	 * handles it, and with interrupts off nothing wakes the vcpu.)
	 * These encode the same in 32- and 64-bit code. */
	mov	$I8042_RESET, %al
	out	%al, $I8042_COMMAND
1:	hlt
	jmp	1b

#if PAGING
	.section .rodata
	.align 8
gdt:
	.quad	0
	.quad	0x00af9a000000ffff	/* GDT_CODE: 64-bit, ring 0 */
	.quad	0x00cf92000000ffff	/* GDT_DATA */
gdt_end:
gdt_ptr:
	.word	gdt_end - gdt - 1
	.long	gdt

	.section .bss
	.align 4096
pml4:	.skip	4096
pdpt:	.skip	4096
pd:	.skip	4096
pt:	.skip	4096
pd_touch:	.skip	4096
pt_touch:	.skip	4096
#endif

	.section .note.GNU-stack, "", @progbits
