# EDD boot strap. Only bytes [0,440) are installed, preserving disk identity,
# partition entries and signature in the existing MBR (including a GPT PMBR).
.code16
.global _start
_start:
    ljmp $0,$real
real:
    cli
    xor %ax,%ax
    mov %ax,%ds
    mov %ax,%es
    mov %ax,%ss
    mov $0x7c00,%sp
    sti
    mov %dl,drive
    mov $0x55aa,%bx
    mov $0x4100,%ax
    int $0x13
    jc fail
    cmp $0xaa55,%bx
    jne fail
    test $1,%cl
    jz fail
    mov drive,%dl
    mov $geometry,%si
    mov $0x4800,%ax
    int $0x13
    jc fail
    cmpw $512,geometry+24
    jne fail
    int $0x12
    cmp $576,%ax
    jb fail
    movl stage_lba,%eax
    movl %eax,dap+8
    movl stage_lba+4,%eax
    movl %eax,dap+12
next:
    movb drive,%dl
    movw $dap,%si
    movw $1,dap+2
    movb $0x42,%ah
    int $0x13
    jc fail
    addw $0x20,dap+6
    addl $1,dap+8
    adcl $0,dap+12
    decw sectors
    jnz next
    mov drive,%dl
    ljmp $0x1000,$0
fail:
    mov $0x0e45,%ax
    int $0x10
    cli
1:  hlt
    jmp 1b
.balign 4
dap: .byte 0x10,0
    .word 1,0,0x1000
    .quad 0
drive: .byte 0
geometry: .word 30
    .space 28
.org 400
.ascii "RBBOOT01"
stage_lba: .quad 0
sectors: .word 0
fat_lba: .quad 0
fat_sectors: .quad 0
.org 440
