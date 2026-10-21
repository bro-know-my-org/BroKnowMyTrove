//! Reproducible generated properties; deliberately bounded for ordinary CI.
use crate::{
    CreateOptions, Credential, Entry, EntryKind, Error, Id, OpenMode, Vault, codec,
    crypto::{KdfParams, Password, Secret},
    keyfile::KeyFile,
    ledger::Commit,
    model::{CHUNK, Snapshot},
    operation::{Operation, Point},
    stream,
};
use std::io::{self, Cursor};

// Test data only. Never use this deterministic generator for keys or production IDs.
struct Generator(u64);
impl Generator {
    fn next(&mut self) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 as usize
    }
}

#[test]
fn generated_streams_authenticate_mutation_truncation_identity_and_record_replay() {
    let master = Secret::generate().unwrap();
    let vault = Id([1; 16]);
    let object = Id([2; 16]);
    let mut rng = Generator(0x6d36_7061_796c_6f61);
    for case in 0..48 {
        let length = match case {
            0 => 0,
            1 => CHUNK - 1,
            2 => CHUNK,
            3 => CHUNK + 1,
            4 => CHUNK * 2,
            _ => rng.next() % 4096,
        };
        let original: Vec<u8> = (0..length).map(|_| rng.next() as u8).collect();
        let mut encrypted = Vec::new();
        stream::encrypt(
            &master,
            vault,
            object,
            length as u64,
            &mut Cursor::new(&original),
            &mut encrypted,
            &Operation::default(),
        )
        .unwrap();
        let mut plain = Vec::new();
        stream::decrypt(
            &master,
            vault,
            object,
            length as u64,
            &mut Cursor::new(&encrypted),
            &mut plain,
            &Operation::default(),
        )
        .unwrap();
        assert_eq!(plain, original, "case {case}");
        for mutation in 0..16 {
            let mut damaged = encrypted.clone();
            match mutation % 4 {
                0 => {
                    let index = rng.next() % damaged.len();
                    damaged[index] ^= 1 << (rng.next() % 8);
                }
                1 => damaged.truncate(rng.next() % damaged.len()),
                2 => damaged.push(rng.next() as u8),
                _ => {
                    // Forge the first frame length, including integer extremes.
                    damaged[104..108].copy_from_slice(&(rng.next() as u32).to_le_bytes());
                }
            }
            assert!(
                stream::decrypt(
                    &master,
                    vault,
                    object,
                    length as u64,
                    &mut Cursor::new(&damaged),
                    &mut io::sink(),
                    &Operation::default()
                )
                .is_err(),
                "case {case}, mutation {mutation}"
            );
        }
        for (v, o, n) in [
            (object, object, length as u64),
            (vault, vault, length as u64),
            (vault, object, length as u64 + 1),
        ] {
            assert!(
                stream::decrypt(
                    &master,
                    v,
                    o,
                    n,
                    &mut Cursor::new(&encrypted),
                    &mut io::sink(),
                    &Operation::default()
                )
                .is_err()
            );
        }
        if length == CHUNK * 2 {
            let frame = CHUNK + 21;
            let mut replay = encrypted.clone();
            replay.copy_within(104..104 + frame, 104 + frame);
            assert!(
                stream::decrypt(
                    &master,
                    vault,
                    object,
                    length as u64,
                    &mut Cursor::new(replay),
                    &mut io::sink(),
                    &Operation::default()
                )
                .is_err()
            );
        }
    }
}

#[test]
fn seeded_parser_mutations_remain_bounded_and_valid_values_roundtrip() {
    let corpus = [
        br#"{"entries":[]}"#.as_slice(),
        br#"{"sequence":0,"parent":null,"snapshot":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}"#,
        br#"{"magic":"BKMT-1","vault_id":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"kdf":{"operations":1,"memory_bytes":8388608},"salt":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"password":[],"recovery":null,"authentication":[]}"#,
        include_bytes!("../tests/vectors/metadata.json"),
    ];
    let mut rng = Generator(0x6d36_7061_7273_6572);
    let mut accepted = [0usize; 3];
    for case in 0..4096 {
        let mut bytes = corpus[case % corpus.len()].to_vec();
        // Keep the first corpus pass intact to exercise successful parse paths too.
        let mutations = if case < corpus.len() {
            0
        } else {
            1 + rng.next() % 8
        };
        for _ in 0..mutations {
            let index = rng.next() % bytes.len().max(1);
            match rng.next() % 4 {
                0 if !bytes.is_empty() => bytes[index] ^= 1 << (rng.next() % 8),
                1 => bytes.insert(index, rng.next() as u8),
                2 => bytes.truncate(index),
                _ => bytes.extend_from_slice(b"\0{}[]"),
            }
        }
        accepted[0] += usize::from(codec::decode::<Commit>(&bytes, codec::COMMIT_LIMIT).is_ok());
        accepted[1] += usize::from(codec::decode::<KeyFile>(&bytes, codec::KEY_LIMIT).is_ok());
        if let Ok(snapshot) = codec::decode::<Snapshot>(&bytes, codec::SNAPSHOT_LIMIT) {
            accepted[2] += 1;
            let _ = snapshot.validate();
            let encoded = codec::encode(&snapshot, codec::SNAPSHOT_LIMIT).unwrap();
            let decoded: Snapshot = codec::decode(&encoded, codec::SNAPSHOT_LIMIT).unwrap();
            assert_eq!(
                *encoded,
                *codec::encode(&decoded, codec::SNAPSHOT_LIMIT).unwrap()
            );
        }
    }
    assert!(accepted.iter().all(|count| *count > 0));
    assert!(matches!(
        codec::decode::<KeyFile>(&vec![0; codec::KEY_LIMIT + 1], codec::KEY_LIMIT),
        Err(Error::ResourceLimit)
    ));
    assert!(codec::decode::<Snapshot>(&b"[".repeat(1024), codec::SNAPSHOT_LIMIT).is_err());
}

#[test]
fn generated_trees_preserve_subtrees_and_reject_cycles_missing_parents_and_duplicate_ids() {
    let mut rng = Generator(0x6d36_7472_6565_6d6f);
    for _ in 0..64 {
        let mut snapshot = Snapshot::default();
        for i in 0..64u128 {
            snapshot.entries.push(Entry {
                id: Id((i + 1).to_le_bytes()),
                parent: if i == 0 || rng.next().is_multiple_of(4) {
                    None
                } else {
                    Some(snapshot.entries[rng.next() % i as usize].id)
                },
                name: format!("目录-café-{i}"),
                trash_group: None,
                kind: EntryKind::Directory,
            });
        }
        snapshot.validate().unwrap();
        let root = snapshot.entries[rng.next() % snapshot.entries.len()].id;
        let subtree = snapshot.subtree(root);
        for entry in &snapshot.entries {
            let mut ancestor = Some(entry.id);
            let mut expected = false;
            while let Some(id) = ancestor {
                expected |= id == root;
                ancestor = snapshot.entries.iter().find(|e| e.id == id).unwrap().parent;
            }
            assert_eq!(subtree.contains(&entry.id), expected);
        }
        for invalid in 0..3 {
            let mut damaged = snapshot.clone();
            match invalid {
                0 => damaged.entries[0].parent = Some(damaged.entries[0].id),
                1 => damaged.entries[0].parent = Some(Id([255; 16])),
                _ => damaged.entries[1].id = damaged.entries[0].id,
            }
            assert!(damaged.validate().is_err());
        }
    }
}

#[test]
fn generated_recovery_sequences_match_an_independent_old_or_new_state_model() {
    for seed in 1..=8 {
        let mut rng = Generator(seed);
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("vault");
        let password = Password::new("disposable generated recovery".into()).unwrap();
        let (mut vault, _) = Vault::create(
            &path,
            &password,
            CreateOptions {
                kdf: KdfParams {
                    operations: 1,
                    memory_bytes: 8 * 1024 * 1024,
                },
                recovery_key: false,
            },
        )
        .unwrap();
        let mut model = Vec::new();
        for i in 0..4 {
            let name = format!("root-{i}");
            let id = vault.mkdir(None, &name, &Operation::default()).unwrap();
            model.push((id, name, false));
        }
        for step in 0..24 {
            let index = rng.next() % model.len();
            let (id, _, trashed) = &model[index];
            let mut next = model.clone();
            let rename = !trashed && rng.next().is_multiple_of(2);
            if rename {
                next[index].1 = format!("renamed-{index}-{step}");
            } else {
                next[index].2 = !trashed;
            }
            let point = [
                Point::SnapshotWritten,
                Point::SnapshotPublished,
                Point::CommitSynced,
                Point::CommitPublished,
                Point::CommitDirectorySynced,
            ][rng.next() % 5];
            let cancel = rng.next().is_multiple_of(2);
            let op = if cancel {
                Operation::default().cancelling_at(point)
            } else {
                Operation::default().failing_at(point)
            };
            let published = matches!(point, Point::CommitPublished | Point::CommitDirectorySynced);
            let old_head = vault.current_commit();
            let result = if rename {
                vault.rename(*id, &next[index].1, &op)
            } else if *trashed {
                vault.restore_entry(*id, &op)
            } else {
                vault.trash_entry(*id, &op)
            };
            assert_eq!(
                result.is_ok(),
                cancel && published,
                "seed {seed}, step {step}"
            );
            if published {
                model = next;
            }
            drop(vault);
            vault =
                Vault::open(&path, Credential::Password(&password), OpenMode::ReadWrite).unwrap();
            assert_eq!(vault.current_commit() != old_head, published);
            assert_eq!(vault.entries().count(), model.len());
            for (id, name, trashed) in &model {
                let entry = vault.entry(*id).unwrap();
                assert_eq!(&entry.name, name, "seed {seed}, step {step}");
                assert_eq!(entry.is_trashed(), *trashed);
                assert_eq!(entry.parent, None);
            }
        }
    }
}
