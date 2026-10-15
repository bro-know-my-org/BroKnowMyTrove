//! This module writes plaintext only into a private sink owned by the export transaction.
use crate::{
    Error, Id, Result, codec,
    crypto::{self, Secret},
    model::{CHUNK, object_size},
    operation::Operation,
};
use alkali::symmetric::cipher_stream::xchacha20poly1305 as sodium;
use std::io::{Read, Write};
use zeroize::Zeroizing;

pub(crate) fn encrypt(
    master: &Secret,
    vault: Id,
    object: Id,
    length: u64,
    source: &mut impl Read,
    target: &mut impl Write,
    operation: &Operation,
) -> Result<()> {
    object_size(length)?;
    operation.check()?;
    let context = codec::aad(3, vault, object);
    let secret = Secret::generate()?;
    let wrapped = crypto::seal(&master.derive(b"BKMFILE1")?, &context, secret.0.as_slice())?;
    let key = sodium::Key::try_from(secret.0.as_slice())?;
    let mut stream = sodium::EncryptionStream::new(&key)?;
    target.write_all(b"BKMTSTR1")?;
    target.write_all(&wrapped)?;
    target.write_all(&stream.get_header())?;
    let mut plaintext = Zeroizing::new(vec![0; CHUNK]);
    let mut ciphertext = vec![0; CHUNK + sodium::OVERHEAD_LENGTH];
    let mut processed = 0;
    let mut index = 0u64;
    while processed < length {
        operation.check()?;
        let count = (length - processed).min(CHUNK as u64) as usize;
        source
            .read_exact(&mut plaintext[..count])
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::UnexpectedEof {
                    Error::SourceChanged
                } else {
                    error.into()
                }
            })?;
        let aad = record_aad(&context, index);
        let written = stream.encrypt(&plaintext[..count], Some(&aad), &mut ciphertext)?;
        target.write_all(&(written as u32).to_le_bytes())?;
        target.write_all(&ciphertext[..written])?;
        processed += count as u64;
        index += 1;
        operation.progress(processed, length);
    }
    let mut extra = [0];
    if source.read(&mut extra)? != 0 {
        return Err(Error::SourceChanged);
    }
    operation.check()?;
    let written = stream.finalise(&[], Some(&record_aad(&context, index)), &mut ciphertext)?;
    target.write_all(&(written as u32).to_le_bytes())?;
    target.write_all(&ciphertext[..written])?;
    Ok(())
}

pub(crate) fn decrypt(
    master: &Secret,
    vault: Id,
    object: Id,
    length: u64,
    source: &mut impl Read,
    private_target: &mut impl Write,
    operation: &Operation,
) -> Result<()> {
    object_size(length)?;
    let mut header = [0; 104];
    source.read_exact(&mut header)?;
    if &header[..8] != b"BKMTSTR1" {
        return Err(Error::InvalidFormat);
    }
    let context = codec::aad(3, vault, object);
    let unwrapped = crypto::unseal(&master.derive(b"BKMFILE1")?, &context, &header[8..80])?;
    if unwrapped.len() != 32 {
        return Err(Error::InvalidFormat);
    }
    let key = sodium::Key::try_from(unwrapped.as_slice())?;
    let stream_header = header[80..].try_into().map_err(|_| Error::InvalidFormat)?;
    let mut stream = sodium::DecryptionStream::new(&key, &stream_header)?;
    let mut ciphertext = vec![0; CHUNK + sodium::OVERHEAD_LENGTH];
    let mut plaintext = Zeroizing::new(vec![0; CHUNK]);
    let mut processed = 0;
    let mut index = 0;
    loop {
        operation.check()?;
        let mut frame_length = [0; 4];
        source.read_exact(&mut frame_length)?;
        let frame_length = u32::from_le_bytes(frame_length) as usize;
        let expected = (length - processed).min(CHUNK as u64) as usize;
        if frame_length != expected + sodium::OVERHEAD_LENGTH {
            return Err(Error::InvalidFormat);
        }
        source.read_exact(&mut ciphertext[..frame_length])?;
        let (tag, count) = stream
            .decrypt(
                &ciphertext[..frame_length],
                Some(&record_aad(&context, index)),
                &mut plaintext,
            )
            .map_err(|_| Error::Authentication)?;
        if expected == 0 {
            if tag != sodium::MessageType::Final || count != 0 {
                return Err(Error::InvalidFormat);
            }
            let mut extra = [0];
            if source.read(&mut extra)? != 0 {
                return Err(Error::InvalidFormat);
            }
            return Ok(());
        }
        if tag != sodium::MessageType::Message || count != expected {
            return Err(Error::InvalidFormat);
        }
        private_target.write_all(&plaintext[..count])?;
        processed += count as u64;
        index += 1;
        operation.progress(processed, length);
    }
}

fn record_aad(context: &[u8], index: u64) -> Vec<u8> {
    let mut aad = context.to_vec();
    aad.extend_from_slice(&index.to_le_bytes());
    aad
}

#[cfg(test)]
mod tests;
