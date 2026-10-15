//! Small safe interface to libsodium. No cryptographic primitive is implemented here.
use alkali::{
    hash::{generic, kdf::blake2b as kdf, pbkdf::argon2id},
    random,
    symmetric::aead::xchacha20poly1305_ietf as aead,
};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use crate::{Error, Result};

pub const PASSWORD_MAX: usize = 4096;

/// NFC/UTF-8, without trimming or case conversion; no Debug/Display implementation.
pub struct Password(Zeroizing<String>);

impl Password {
    pub fn new(input: String) -> Result<Self> {
        let input = Zeroizing::new(input);
        if input.is_empty() || input.len() > PASSWORD_MAX {
            return Err(Error::InvalidInput);
        }
        let normalized = Zeroizing::new(input.nfc().collect::<String>());
        if normalized.len() > PASSWORD_MAX {
            return Err(Error::InvalidInput);
        }
        Ok(Self(normalized))
    }

    pub fn matches(&self, other: &Self) -> bool {
        alkali::mem::eq(self.0.as_bytes(), other.0.as_bytes()).unwrap_or(false)
    }
}

/// 32-byte secret, zeroed on drop; never serializable or printable implicitly.
pub struct Secret(pub(crate) Zeroizing<[u8; 32]>);

impl Secret {
    pub fn generate() -> Result<Self> {
        Ok(Self(Zeroizing::new(random_array()?)))
    }

    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self(Zeroizing::new(
            bytes.try_into().map_err(|_| Error::InvalidFormat)?,
        )))
    }

    pub(crate) fn derive(&self, context: &[u8; 8]) -> Result<Self> {
        let key = kdf::Key::try_from(self.0.as_slice())?;
        let mut output = Self(Zeroizing::new([0; 32]));
        kdf::derive_subkey(&key, context, 1, output.0.as_mut_slice())?;
        Ok(output)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KdfParams {
    pub operations: u32,
    pub memory_bytes: u64,
}

impl Default for KdfParams {
    fn default() -> Self {
        Self {
            operations: 3,
            memory_bytes: 256 * 1024 * 1024,
        }
    }
}

impl KdfParams {
    /// Validate BEFORE allocating or invoking Argon2id on untrusted key information.
    pub fn validate(self) -> Result<()> {
        if !(1..=10).contains(&self.operations)
            || !(8 * 1024 * 1024..=1024 * 1024 * 1024).contains(&self.memory_bytes)
        {
            return Err(Error::ResourceLimit);
        }
        Ok(())
    }

    pub(crate) fn derive(self, password: &Password, salt: &[u8; 16]) -> Result<Secret> {
        self.validate()?;
        let mut key = Secret(Zeroizing::new([0; 32]));
        argon2id::derive_key(
            password.0.as_bytes(),
            salt,
            self.operations as usize,
            usize::try_from(self.memory_bytes).map_err(|_| Error::ResourceLimit)?,
            key.0.as_mut_slice(),
        )?;
        Ok(key)
    }
}

pub(crate) fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut output = [0; N];
    random::fill_random(&mut output)?;
    Ok(output)
}

pub(crate) fn digest(bytes: &[u8]) -> Result<[u8; 32]> {
    Ok(generic::hash(bytes, None)?)
}

/// Binary AEAD envelope: nonce[24] followed by ciphertext and tag[16].
pub(crate) fn seal(key: &Secret, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let key = aead::Key::try_from(key.0.as_slice())?;
    let nonce = random_array::<24>()?;
    let mut ciphertext = vec![0; 24 + plaintext.len() + aead::MAC_LENGTH];
    ciphertext[..24].copy_from_slice(&nonce);
    aead::encrypt(
        plaintext,
        Some(aad),
        &key,
        Some(&nonce),
        &mut ciphertext[24..],
    )?;
    Ok(ciphertext)
}

pub(crate) fn unseal(key: &Secret, aad: &[u8], ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if ciphertext.len() < 40 {
        return Err(Error::InvalidFormat);
    }
    let key = aead::Key::try_from(key.0.as_slice())?;
    let nonce = ciphertext[..24]
        .try_into()
        .map_err(|_| Error::InvalidFormat)?;
    let mut plaintext = Zeroizing::new(vec![0; ciphertext.len() - 40]);
    aead::decrypt(&ciphertext[24..], Some(aad), &key, &nonce, &mut plaintext)
        .map_err(|_| Error::Authentication)?;
    Ok(plaintext)
}

/// Independent recovery key, with a four-byte BLAKE2b checksum (not authentication).
pub struct RecoveryKey(Secret);

impl RecoveryKey {
    pub fn generate() -> Result<Self> {
        Ok(Self(Secret::generate()?))
    }

    pub fn encode(&self) -> Result<Zeroizing<String>> {
        let checksum = digest(self.0.0.as_slice())?;
        let encoded = Zeroizing::new(hex::encode(self.0.0.as_slice()));
        Ok(Zeroizing::new(format!(
            "{}-{}",
            *encoded,
            hex::encode(&checksum[..4])
        )))
    }

    pub fn decode(input: &str) -> Result<Self> {
        if input.len() != 73 || input.as_bytes()[64] != b'-' || !input.is_ascii() {
            return Err(Error::InvalidInput);
        }
        let bytes = Zeroizing::new(hex::decode(&input[..64]).map_err(|_| Error::InvalidInput)?);
        let checksum = hex::decode(&input[65..]).map_err(|_| Error::InvalidInput)?;
        if checksum != digest(&bytes)?[..4] {
            return Err(Error::InvalidInput);
        }
        Ok(Self(Secret::from_bytes(&bytes)?))
    }

    pub(crate) fn secret(&self) -> &Secret {
        &self.0
    }
}

#[cfg(test)]
mod tests;
