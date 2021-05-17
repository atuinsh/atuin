ifndef X64
.686p
.XMM
.model flat, C
endif

.code

rust_crypto_util_supports_aesni PROC public
  ; Return false since the AES-NI function have not been
  ; converted to assembly
  xor eax, eax
  ret
rust_crypto_util_supports_aesni ENDP

; The rust_crypto_util_fixed_time_eq_asm for X86-64
ifdef X64
rust_crypto_util_fixed_time_eq_asm PROC public lhs:QWORD, rhs:QWORD, count:QWORD
  ; lhs is in RCX
  ; rhs is in RDX
  ; count is in R8

  ; set the return value initially to 0
  xor eax, eax

  test r8, r8
  jz DONE

  BEGIN:

  mov r10b, [rcx]
  xor r10b, [rdx]
  or al, r10b

  inc rcx
  inc rdx
  dec r8
  jnz BEGIN

  DONE:

  ret
rust_crypto_util_fixed_time_eq_asm ENDP
endif

; The rust_crypto_util_fixed_time_eq_asm for X86 (32-bit)
ifndef X64
rust_crypto_util_fixed_time_eq_asm PROC public lhs:DWORD, rhs:DWORD, count:DWORD
  push ebx
  push esi

  mov ecx, lhs
  mov edx, rhs
  mov esi, count

  xor eax, eax

  test esi, esi
  jz DONE

  BEGIN:

  mov bl, [ecx]
  xor bl, [edx]
  or al, bl

  inc ecx
  inc edx
  dec esi
  jnz BEGIN

  DONE:

  pop esi
  pop ebx

  ret
rust_crypto_util_fixed_time_eq_asm ENDP
endif

ifdef X64
rust_crypto_util_secure_memset PROC public dst:QWORD, val:BYTE, count:QWORD
  ; dst is in RCX
  ; val is in RDX
  ; count is in R8

  test r8, r8
  jz DONE

  BEGIN:

  mov [rcx], dl
  inc rcx
  dec r8
  jnz BEGIN

  DONE:

  ret
rust_crypto_util_secure_memset ENDP
endif

ifndef X64
rust_crypto_util_secure_memset PROC public dst:DWORD, val:BYTE, count:DWORD
  mov eax, dst
  mov cl, val
  mov edx, count

  test edx, edx
  jz DONE

  BEGIN:

  mov [eax], cl
  inc eax
  dec edx
  jnz BEGIN

  DONE:

  ret
rust_crypto_util_secure_memset ENDP
endif

end

