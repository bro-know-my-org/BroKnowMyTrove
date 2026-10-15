use super::*;
use std::io::{self, Cursor};

#[test]
fn roundtrip_boundaries_and_reject_damage_swap_truncation_and_tail() {
    let key = Secret::generate().unwrap();
    let vault = Id([1; 16]);
    let object = Id([2; 16]);
    for length in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, CHUNK * 2] {
        let original = vec![123; length];
        let mut encrypted = Vec::new();
        encrypt(
            &key,
            vault,
            object,
            length as u64,
            &mut Cursor::new(&original),
            &mut encrypted,
            &Operation::default(),
        )
        .unwrap();
        assert_eq!(encrypted.len() as u64, object_size(length as u64).unwrap());
        let mut plaintext = Vec::new();
        decrypt(
            &key,
            vault,
            object,
            length as u64,
            &mut Cursor::new(&encrypted),
            &mut plaintext,
            &Operation::default(),
        )
        .unwrap();
        assert_eq!(plaintext, original);
        for damaged in [
            encrypted[..encrypted.len() - 1].to_vec(),
            [encrypted.as_slice(), &[0]].concat(),
            {
                let mut damaged = encrypted.clone();
                damaged[90] ^= 1;
                damaged
            },
        ] {
            assert!(
                decrypt(
                    &key,
                    vault,
                    object,
                    length as u64,
                    &mut Cursor::new(damaged),
                    &mut io::sink(),
                    &Operation::default()
                )
                .is_err()
            );
        }
        assert!(
            decrypt(
                &key,
                vault,
                Id([3; 16]),
                length as u64,
                &mut Cursor::new(&encrypted),
                &mut io::sink(),
                &Operation::default()
            )
            .is_err()
        );
        assert!(
            decrypt(
                &key,
                vault,
                object,
                length as u64 + 1,
                &mut Cursor::new(&encrypted),
                &mut io::sink(),
                &Operation::default()
            )
            .is_err()
        );
    }
}

struct ShortWriter(Vec<u8>);
impl Write for ShortWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = bytes.len().min(7);
        self.0.extend_from_slice(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn short_writes_and_source_length_changes_are_not_success() {
    let key = Secret::generate().unwrap();
    let mut output = ShortWriter(Vec::new());
    encrypt(
        &key,
        Id([1; 16]),
        Id([2; 16]),
        100,
        &mut Cursor::new(vec![7; 100]),
        &mut output,
        &Operation::default(),
    )
    .unwrap();
    let mut recovered = Vec::new();
    decrypt(
        &key,
        Id([1; 16]),
        Id([2; 16]),
        100,
        &mut Cursor::new(output.0),
        &mut recovered,
        &Operation::default(),
    )
    .unwrap();
    assert_eq!(recovered, vec![7; 100]);
    for actual in [99, 101] {
        assert!(
            encrypt(
                &key,
                Id([1; 16]),
                Id([2; 16]),
                100,
                &mut Cursor::new(vec![7; actual]),
                &mut io::sink(),
                &Operation::default()
            )
            .is_err()
        );
    }
}

#[test]
fn huge_source_is_bounded_and_cancellable_without_materializing_it() {
    struct BudgetReader {
        consumed: usize,
    }
    impl Read for BudgetReader {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if self.consumed + bytes.len() > 3 * CHUNK {
                return Err(io::Error::other("cancellation read budget exceeded"));
            }
            bytes.fill(17);
            self.consumed += bytes.len();
            Ok(bytes.len())
        }
    }
    let operation = Operation::default();
    let cancel = operation.clone();
    let operation = operation.with_progress(move |progress| {
        if progress.processed >= 2 * CHUNK as u64 {
            cancel.cancel();
        }
    });
    let key = Secret::generate().unwrap();
    let mut source = BudgetReader { consumed: 0 };
    let result = encrypt(
        &key,
        Id([1; 16]),
        Id([2; 16]),
        100 * 1024 * 1024 * 1024,
        &mut source,
        &mut io::sink(),
        &operation,
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(source.consumed, 2 * CHUNK);
}
