/* Loaded by the VMM's PVH ELF loader at the physical addresses below. */
OUTPUT_FORMAT(elf64-x86-64)
ENTRY(pvh_start)

SECTIONS
{
	. = 0x100000;

	.text : { *(.text.boot) *(.text .text.*) }
	.rodata : { *(.rodata .rodata.*) }
	.notes : { KEEP(*(.notes)) }

	.bss : ALIGN(4096) {
		__bss_start = .;
		*(.bss .bss.*)
		__bss_end = .;
	}
	_end = .;

	/* boot.S's TOUCH_PA must stay past the image. */
	ASSERT(_end <= 0x200000, "image overlaps TOUCH_PA")

	/DISCARD/ : { *(.comment) *(.eh_frame*) *(.note.*) }
}
