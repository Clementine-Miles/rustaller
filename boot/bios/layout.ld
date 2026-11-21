ENTRY(_start)
SECTIONS {
    . = 0x10000;
    .entry : { KEEP(*(.entry)) }
    .text : { *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }
    .data : { *(.data .data.* .data.rel.ro* .got*) }
    .bss (NOLOAD) : { __bss_start = .; *(.bss .bss.* COMMON) __bss_end = .; }
    ASSERT(. < 0x79000, "BIOS secondary loader exceeds conventional memory budget")
    /DISCARD/ : { *(.note* .comment* .eh_frame* .gcc_except_table* .interp .dynamic .dynsym .dynstr .gnu*) }
}
