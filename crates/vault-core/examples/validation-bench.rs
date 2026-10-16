//! Bounded baseline, not a complete performance acceptance suite.
use serde_json::json;
use std::{
    fs,
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use vault_core::{
    CreateOptions, Credential, Error, OpenMode, Vault, crypto::Password, operation::Operation,
};

fn percentiles(samples: &[Duration]) -> serde_json::Value {
    let mut milliseconds: Vec<_> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    milliseconds.sort_by(f64::total_cmp);
    json!({"samples": milliseconds.len(), "p50_ms": milliseconds[(milliseconds.len() - 1) / 2],
        "p95_ms": milliseconds[(milliseconds.len() * 95).div_ceil(100) - 1]})
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = args.next().ok_or("expected scratch parent directory")?;
    let mib: u64 = args
        .next()
        .ok_or("expected file size in MiB, 8..=1024")?
        .to_str()
        .ok_or("non UTF-8 size")?
        .parse()?;
    if !(8..=1024).contains(&mib) {
        return Err("size must be 8..=1024 MiB".into());
    }
    let length = mib * 1024 * 1024;
    if vault_platform::capacity(std::path::Path::new(&root))?.available
        < length * 3 + 64 * 1024 * 1024
    {
        return Err("insufficient scratch capacity".into());
    }
    let temp = tempfile::Builder::new()
        .prefix("bkm-bench-")
        .tempdir_in(root)?;
    let source = temp.path().join("source");
    let mut file = fs::File::create(&source)?;
    let block = vec![42; 1024 * 1024];
    for _ in 0..mib {
        file.write_all(&block)?;
    }
    file.sync_all()?;
    drop(file);
    let output = temp.path().join("output");
    let started = Instant::now();
    let mut input = fs::File::open(&source)?;
    let mut copied = fs::File::create(&output)?;
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        copied.write_all(&buffer[..count])?;
    }
    copied.sync_all()?;
    drop(copied);
    drop(input);
    let io_seconds = started.elapsed().as_secs_f64();
    fs::remove_file(&output)?;

    let password = Password::new("disposable benchmark 中文 café".into())?;
    let path = temp.path().join("vault");
    let started = Instant::now();
    let (vault, _) = Vault::create(&path, &password, CreateOptions::default())?;
    let create_ms = started.elapsed().as_secs_f64() * 1000.0;
    drop(vault);
    let mut unlocks = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        let vault = Vault::open(&path, Credential::Password(&password), OpenMode::ReadOnly)?;
        unlocks.push(started.elapsed());
        drop(vault);
    }
    let mut vault = Vault::open(&path, Credential::Password(&password), OpenMode::ReadWrite)?;
    let started = Instant::now();
    let id = vault.import_file(None, "payload", &source, &Operation::default())?;
    let import_seconds = started.elapsed().as_secs_f64();
    let started = Instant::now();
    vault.export_file(id, &output, &Operation::default())?;
    let export_seconds = started.elapsed().as_secs_f64();
    // Independent streaming byte comparison; do not load the GiB sample into RAM.
    let mut left = fs::File::open(&source)?;
    let mut right = fs::File::open(&output)?;
    let mut a = vec![0; 1024 * 1024];
    let mut b = vec![0; a.len()];
    loop {
        let n = left.read(&mut a)?;
        right.read_exact(&mut b[..n])?;
        assert_eq!(&a[..n], &b[..n]);
        if n == 0 {
            assert_eq!(right.read(&mut b)?, 0);
            break;
        }
    }
    drop(left);
    drop(right);
    fs::remove_file(output)?;
    let cancellation = Operation::default();
    let signal = cancellation.clone();
    let cancelled_at = Arc::new(Mutex::new(None));
    let observed = cancelled_at.clone();
    let cancellation = cancellation.with_progress(move |_| {
        let mut time = observed.lock().unwrap();
        if time.is_none() {
            *time = Some(Instant::now());
            signal.cancel();
        }
    });
    assert!(matches!(
        vault.import_file(None, "cancelled", &source, &cancellation),
        Err(Error::Cancelled)
    ));
    let cancel_ms = cancelled_at
        .lock()
        .unwrap()
        .ok_or("no active progress")?
        .elapsed()
        .as_secs_f64()
        * 1000.0;
    assert_eq!(vault.entries().count(), 1);
    let mut commits = Vec::new();
    for i in 0..200 {
        let started = Instant::now();
        vault.mkdir(None, &format!("directory-{i}"), &Operation::default())?;
        commits.push(started.elapsed());
    }
    drop(vault);
    let started = Instant::now();
    let vault = Vault::open(&path, Credential::Password(&password), OpenMode::ReadOnly)?;
    let metadata_open_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(vault.entries().count(), 201);
    drop(vault);
    let peak = fs::read_to_string("/proc/self/status").ok().and_then(|s| {
        s.lines()
            .find(|line| line.starts_with("VmHWM:"))
            .map(str::to_owned)
    });
    println!(
        "{}",
        json!({"build": if cfg!(debug_assertions) {"debug"} else {"release"},
        "file_mib": mib, "io_copy_sync_mib_s": mib as f64 / io_seconds,
        "create_ms": create_ms, "empty_unlock": percentiles(&unlocks),
        "import_sync_mib_s": mib as f64 / import_seconds,
        "export_sync_mib_s": mib as f64 / export_seconds,
        "metadata_commits": percentiles(&commits), "metadata_entries": 201,
        "metadata_open_with_kdf_ms": metadata_open_ms,
        "active_cancel_cleanup_ms": cancel_ms, "process_peak_memory": peak})
    );
    temp.close()?;
    Ok(())
}
