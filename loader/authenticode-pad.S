/*
 * Cosmopolitan emits two PE data directories and places Mach-O immediately
 * after the PE section table. Reserve the remaining fourteen directory slots
 * at link time so Authenticode can use IMAGE_DIRECTORY_ENTRY_SECURITY without
 * relocating any other APE header after the fact.
 */
.section .pe.header,"a",@progbits
.balign 8
.zero 112
