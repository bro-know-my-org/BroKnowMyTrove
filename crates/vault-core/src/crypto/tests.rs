use super::*;

#[test]
fn normalization_preserves_meaningful_spaces_and_punctuation() {
    let composed = Password::new("中文 café， 密码 ".into()).unwrap();
    let decomposed = Password::new("中文 cafe\u{301}， 密码 ".into()).unwrap();
    assert!(composed.matches(&decomposed));
    assert!(!composed.matches(&Password::new("中文 café， 密码".into()).unwrap()));
    assert!(!composed.matches(&Password::new("中文 café, 密码 ".into()).unwrap()));
    assert!(Password::new("a".repeat(PASSWORD_MAX + 1)).is_err());
}

#[test]
fn argon2_aead_wrong_password_tampering_and_context_binding() {
    let params = KdfParams {
        operations: 1,
        memory_bytes: 8 * 1024 * 1024,
    };
    let password = Password::new("记住这句中文口令".into()).unwrap();
    let key = params.derive(&password, &[17; 16]).unwrap();
    let ciphertext = seal(&key, b"vault/object/version", b"plaintext").unwrap();
    assert_eq!(
        &**unseal(&key, b"vault/object/version", &ciphertext).unwrap(),
        b"plaintext"
    );
    assert!(unseal(&key, b"different object", &ciphertext).is_err());
    let wrong = params
        .derive(&Password::new("另一句".into()).unwrap(), &[17; 16])
        .unwrap();
    assert!(unseal(&wrong, b"vault/object/version", &ciphertext).is_err());
    let mut damaged = ciphertext.clone();
    damaged[30] ^= 1;
    assert!(unseal(&key, b"vault/object/version", &damaged).is_err());
    assert!(
        KdfParams {
            operations: u32::MAX,
            memory_bytes: u64::MAX
        }
        .validate()
        .is_err()
    );
    assert_ne!(
        *key.derive(b"BKMSNAP1").unwrap().0,
        *key.derive(b"BKMFILE1").unwrap().0
    );
}

#[test]
fn recovery_encoding_checks_errors_and_random_keys_differ() {
    let recovery = RecoveryKey::generate().unwrap();
    let encoded = recovery.encode().unwrap();
    assert_eq!(*RecoveryKey::decode(&encoded).unwrap().0.0, *recovery.0.0);
    let mut damaged = encoded.to_string();
    damaged.replace_range(0..1, if &damaged[..1] == "0" { "1" } else { "0" });
    assert!(RecoveryKey::decode(&damaged).is_err());
    assert_eq!(recovery.secret().0.len(), 32);
    assert_ne!(
        *Secret::generate().unwrap().0,
        *Secret::generate().unwrap().0
    );
}

#[test]
fn secretstream_authenticates_order_and_final_record() {
    use alkali::symmetric::cipher_stream::xchacha20poly1305 as stream;
    let key = stream::Key::generate().unwrap();
    let mut encrypt = stream::EncryptionStream::new(&key).unwrap();
    let header = encrypt.get_header();
    let mut first = vec![0; 3 + stream::OVERHEAD_LENGTH];
    encrypt
        .encrypt(b"abc", Some(b"object/0"), &mut first)
        .unwrap();
    let mut last = vec![0; stream::OVERHEAD_LENGTH];
    encrypt.finalise(b"", Some(b"object/1"), &mut last).unwrap();
    let mut decrypt = stream::DecryptionStream::new(&key, &header).unwrap();
    let mut output = [0; 3];
    assert_eq!(
        decrypt
            .decrypt(&first, Some(b"object/0"), &mut output)
            .unwrap()
            .1,
        3
    );
    assert!(!decrypt.is_finalised());
    decrypt.decrypt(&last, Some(b"object/1"), &mut []).unwrap();
    assert!(decrypt.is_finalised());
    let mut reordered = stream::DecryptionStream::new(&key, &header).unwrap();
    assert!(
        reordered
            .decrypt(&last, Some(b"object/1"), &mut [])
            .is_err()
    );
}
