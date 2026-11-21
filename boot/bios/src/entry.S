# Physical addresses used before Linux entry:
# 0x4f00 real stack, 0x5000 E820 records, 0x6000 boot_params, 0x7000 cmdline,
# 0x7c00 boot sector, 0x8000-0xffff EDD bounce, 0x10000 secondary loader,
# 0x79000-0x90000 protected stack. Stage 1 checks INT 12h conventional RAM.
.section .entry,"ax"
.code16
.global _start
_start:
    cli
    movb %dl,%cs:(drive-_start)
    xor %ax,%ax
    mov %ax,%ds
    mov %ax,%es
    mov %ax,%ss
    mov $0x4f00,%esp
    cld
    # Enable A20 via BIOS, then the PC fast A20 gate.
    mov $0x2401,%ax
    int $0x15
    inb $0x92,%al
    and $0xfe,%al
    or $2,%al
    outb %al,$0x92
    xor %ebx,%ebx
    mov $0x5000,%di
e820:
    movl $1,%es:20(%di)
    mov $0xe820,%eax
    mov $0x534d4150,%edx
    mov $24,%ecx
    int $0x15
    jc map_done
    cmp $0x534d4150,%eax
    jne halt16
    cmp $20,%ecx
    jb halt16
    je map_store
    testb $1,%es:20(%di)
    jz map_next
map_store:
    incb %cs:(count-_start)
    add $20,%di
map_next:
    test %ebx,%ebx
    jz map_done
    cmpb $128,%cs:(count-_start)
    jb e820
halt16: cli
    hlt
    jmp halt16
map_done:
    cli
    lgdtl %cs:(gdtr-_start)
    mov %cr0,%eax
    and $0xfffffff3,%eax
    or $3,%eax
    mov %eax,%cr0
    ljmpl $0x10,$protected
.code32
protected:
    mov $0x18,%ax
    mov %ax,%ds
    mov %ax,%es
    mov %ax,%fs
    mov %ax,%gs
    mov %ax,%ss
    mov $0x90000,%esp
    mov $1,%eax
    cpuid
    test $0x4000000,%edx
    jz hang
    mov $0x80000000,%eax
    cpuid
    cmp $0x80000001,%eax
    jb hang
    mov $0x80000001,%eax
    cpuid
    test $0x20000000,%edx
    jz hang
    mov %cr4,%eax
    or $0x600,%eax
    mov %eax,%cr4
    fninit
    cld
    mov $__bss_start,%edi
    mov $__bss_end,%ecx
    sub %edi,%ecx
    xor %eax,%eax
    rep stosb
    sub $12,%esp
    movzbl count,%eax
    push %eax
    call bios_main
hang: cli
    hlt
    jmp hang
# cdecl. Each INT13 call is synchronous, uses the same fixed DAP/bounce area,
# and is invoked only by the single boot processor with interrupts disabled.
.global read_sectors
read_sectors:
    pushal
    mov 36(%esp),%eax
    mov %eax,dap+8
    mov 40(%esp),%eax
    mov %eax,dap+12
    mov 44(%esp),%eax
    mov %ax,dap+2
    movb $0,operation
    jmp switch
.global console_service
console_service:
    pushal
    mov 36(%esp),%eax
    mov %al,operation
    mov 40(%esp),%eax
    mov %al,character
switch:
    mov %esp,protected_sp
    ljmp $0x20,$(transition-_start)
.code16
transition:
    mov $0x28,%ax
    mov %ax,%ds
    mov %ax,%es
    mov %ax,%ss
    mov %cr0,%eax
    and $-2,%eax
    mov %eax,%cr0
    ljmp $0x1000,$(real_read-_start)
real_read:
    xor %ax,%ax
    mov %ax,%ss
    mov $0x4f00,%esp
    mov %ax,%es
    mov %cs,%ax
    mov %ax,%ds
    sti
    cmpb $1,operation-_start
    je keyboard
    cmpb $2,operation-_start
    je teletype
    cmpb $3,operation-_start
    je delay
    mov $(dap-_start),%si
    movb drive-_start,%dl
    movb $0x42,%ah
    sti
    int $0x13
    cli
    setc %cs:(failed-_start)
    jmp restore
keyboard:
    movb $0,failed-_start
    mov $0x0100,%ax
    int $0x16
    jz restore
    xor %ax,%ax
    int $0x16
    mov %al,%cs:(failed-_start)
    jmp restore
teletype:
    movb character-_start,%al
    mov $0x0e,%ah
    mov $7,%bx
    int $0x10
    jmp restore
delay:
    mov $0x8600,%ax
    xor %cx,%cx
    mov $50000,%dx
    int $0x15
restore:
    cli
    lgdtl %cs:(gdtr-_start)
    mov %cr0,%eax
    or $1,%eax
    mov %eax,%cr0
    ljmpl $0x10,$returned
.code32
returned:
    mov $0x18,%ax
    mov %ax,%ss
    mov %ax,%ds
    mov %ax,%es
    mov %ax,%fs
    mov %ax,%gs
    mov protected_sp,%esp
    popal
    movzbl failed,%eax
    cld
    ret
.global enter_linux
enter_linux:
    cli
    cld
    mov 4(%esp),%eax
    mov $0x6000,%esi
    xor %ebp,%ebp
    xor %edi,%edi
    xor %ebx,%ebx
    xor %ecx,%ecx
    xor %edx,%edx
    jmp *%eax
.balign 8
gdt: .quad 0,0,0x00cf9a000000ffff,0x00cf92000000ffff,0x00009a010000ffff,0x000092000000ffff
gdtr: .word .-gdt-1
    .long gdt
.balign 4
dap: .byte 16,0
    .word 0,0x8000,0
    .quad 0
protected_sp: .long 0
count: .byte 0
drive: .byte 0
failed: .byte 0
operation: .byte 0
character: .byte 0
