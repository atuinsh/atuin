ifndef X64
.686p
.XMM
.model flat, C
endif

.code

rust_crypto_aesni_aesimc PROC public
  ret
rust_crypto_aesni_aesimc ENDP

rust_crypto_aesni_setup_working_key_128 PROC public
  ret
rust_crypto_aesni_setup_working_key_128 ENDP

rust_crypto_aesni_setup_working_key_192 PROC public
  ret
rust_crypto_aesni_setup_working_key_192 ENDP

rust_crypto_aesni_setup_working_key_256 PROC public
  ret
rust_crypto_aesni_setup_working_key_256 ENDP

rust_crypto_aesni_encrypt_block PROC public
  ret
rust_crypto_aesni_encrypt_block ENDP

rust_crypto_aesni_decrypt_block PROC public
  ret
rust_crypto_aesni_decrypt_block ENDP

end

