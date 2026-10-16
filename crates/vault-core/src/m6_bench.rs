//! Opt-in release baselines. These are not CI pass/fail performance thresholds.
use crate::{
    CreateOptions, Credential, Entry, EntryKind, Id, OpenMode, Vault,
    crypto::{Password, Secret},
    model::Snapshot,
    operation::Operation,
    stream,
};
use serde_json::json;
use std::{
    io::Cursor,
    time::{Duration, Instant},
};

fn percentiles(samples: &[Duration]) -> serde_json::Value {
    let mut ms: Vec<_> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    json!({"samples": ms.len(), "p50_ms": ms[(ms.len() - 1) / 2],
        "p95_ms": ms[(ms.len() * 95).div_ceil(100) - 1]})
}

#[test]
#[ignore = "opt-in release memory crypto benchmark"]
fn memory_crypto_baseline() {
    let master = Secret::generate().unwrap();
    let input = vec![173; 64 * 1024 * 1024];
    let mut encrypted = Vec::with_capacity(input.len() + 4096);
    let mut plain = Vec::with_capacity(input.len());
    let mut encryption = Vec::new();
    let mut decryption = Vec::new();
    for _ in 0..8 {
        encrypted.clear();
        plain.clear();
        let started = Instant::now();
        stream::encrypt(
            &master,
            Id([1; 16]),
            Id([2; 16]),
            input.len() as u64,
            &mut Cursor::new(&input),
            &mut encrypted,
            &Operation::default(),
        )
        .unwrap();
        encryption.push(started.elapsed());
        let started = Instant::now();
        stream::decrypt(
            &master,
            Id([1; 16]),
            Id([2; 16]),
            input.len() as u64,
            &mut Cursor::new(&encrypted),
            &mut plain,
            &Operation::default(),
        )
        .unwrap();
        decryption.push(started.elapsed());
        assert_eq!(plain, input);
    }
    println!(
        "{}",
        json!({"benchmark": "memory_crypto", "release": !cfg!(debug_assertions),
        "sample_mib": 64, "encrypt": percentiles(&encryption), "decrypt": percentiles(&decryption),
        "scope": "secretstream framing and key wrapping, reused memory buffers, no KDF or filesystem"})
    );
}

#[test]
#[ignore = "opt-in release metadata scale benchmark"]
fn metadata_scale_baseline() {
    let count: usize = std::env::var("BKM_BENCH_ENTRIES")
        .unwrap_or("10000".into())
        .parse()
        .unwrap();
    assert!((1_000..=100_000).contains(&count));
    let parent = std::env::var_os("BKM_BENCH_PARENT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    assert!(
        vault_platform::capacity(&parent).unwrap().available
            > count as u64 * 4096 + 64 * 1024 * 1024
    );
    let temp = tempfile::Builder::new()
        .prefix("bkm-metadata-")
        .tempdir_in(parent)
        .unwrap();
    let path = temp.path().join("vault");
    let password = Password::new("disposable metadata benchmark 中文".into()).unwrap();
    let (mut vault, _) = Vault::create(&path, &password, CreateOptions::default()).unwrap();
    // Seed one authenticated snapshot through the real commit path. Avoid O(n^2)
    // fixture construction; this does not measure n successive mkdir operations.
    let snapshot = Snapshot {
        entries: (0..count)
            .map(|i| Entry {
                id: Id((i as u128 + 1).to_le_bytes()),
                parent: (i >= 128).then(|| Id((i as u128 % 128 + 1).to_le_bytes())),
                name: format!("目录-café-{i}"),
                trash_group: None,
                kind: EntryKind::Directory,
            })
            .collect(),
    };
    let target = snapshot.entries.last().unwrap().id;
    let started = Instant::now();
    vault.commit(snapshot, &Operation::default()).unwrap();
    let fixture_commit_ms = started.elapsed().as_secs_f64() * 1000.0;
    drop(vault);
    let mut opens = Vec::new();
    let mut searches = Vec::new();
    let mut renames = Vec::new();
    for sample in 0..5 {
        let started = Instant::now();
        let mut vault =
            Vault::open(&path, Credential::Password(&password), OpenMode::ReadWrite).unwrap();
        opens.push(started.elapsed());
        assert_eq!(vault.entries().count(), count);
        let started = Instant::now();
        assert_eq!(vault.search("cafe\u{301}", false).count(), count);
        searches.push(started.elapsed());
        let started = Instant::now();
        vault
            .rename(
                target,
                &format!("重命名-café-{sample}"),
                &Operation::default(),
            )
            .unwrap();
        renames.push(started.elapsed());
        assert_eq!(
            vault.entry(target).unwrap().name,
            format!("重命名-café-{sample}")
        );
    }
    let vault = Vault::open(&path, Credential::Password(&password), OpenMode::ReadOnly).unwrap();
    assert_eq!(vault.entry(target).unwrap().name, "重命名-café-4");
    assert!(!vault.is_writable());
    drop(vault);
    println!(
        "{}",
        json!({"benchmark": "metadata_scale", "release": !cfg!(debug_assertions),
        "entries": count, "fixture_commit_ms": fixture_commit_ms, "open_with_kdf": percentiles(&opens),
        "unicode_search": percentiles(&searches), "rename_commit": percentiles(&renames),
        "scope": "128 roots, remaining directories at depth 1; bulk fixture, short history, no file objects or GUI"})
    );
    if std::env::var("BKM_BENCH_KEEP").is_ok_and(|value| value == "1") {
        eprintln!("M6_GUI_FIXTURE={}", temp.keep().display());
    } else {
        temp.close().unwrap();
    }
}
