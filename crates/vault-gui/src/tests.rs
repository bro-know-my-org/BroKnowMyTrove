use super::*;
use std::{path::Path, thread, time::Instant};
use vault_core::{Credential, OpenMode, Vault, crypto::KdfParams, operation::Operation};

fn wait(app: &mut VaultApp, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.task.is_some() {
        assert!(Instant::now() < deadline, "GUI worker did not finish");
        app.poll(ctx);
        thread::sleep(Duration::from_millis(2));
    }
}

fn create(app: &mut VaultApp, path: &Path, ctx: &egui::Context) {
    let options = CreateOptions {
        kdf: KdfParams {
            operations: 1,
            memory_bytes: 8 * 1024 * 1024,
        },
        recovery_key: true,
    };
    app.task = Some(
        Task::start(
            path.into(),
            Request::Create(Password::new("中文café".into()).unwrap(), options),
            ctx.clone(),
        )
        .unwrap(),
    );
}

#[test]
fn gui_session_roundtrip_lock_wrong_password_recovery_and_idle_clear() {
    let ctx = egui::Context::default();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("vault");
    let mut app = VaultApp::default();
    create(&mut app, &path, &ctx);
    wait(&mut app, &ctx);
    let session = app.session.as_mut().unwrap();
    let recovery = RecoveryKey::decode(session.recovery.as_ref().unwrap()).unwrap();
    session
        .vault
        .mkdir(None, "资料目录", &Operation::default())
        .unwrap();
    assert!(matches!(
        Vault::open(&path, Credential::Recovery(&recovery), OpenMode::ReadOnly),
        Err(Error::Locked)
    ));
    app.browser.query = Zeroizing::new("资料".into());
    app.form.phrase = Zeroizing::new("待清除口令".into());
    app.tick(
        &ctx,
        app.last_activity.unwrap() + Duration::from_secs(301),
        false,
        false,
    );
    assert!(app.session.is_none());
    assert!(app.browser.query.is_empty() && app.form.phrase.is_empty());
    app.task = Some(
        Task::start(
            path.clone(),
            Request::Open(Password::new("错误".into()).unwrap()),
            ctx.clone(),
        )
        .unwrap(),
    );
    wait(&mut app, &ctx);
    assert!(app.session.is_none());
    assert!(app.notice.contains("authentication failed"));
    app.task = Some(Task::start(path.clone(), Request::Recover(recovery), ctx.clone()).unwrap());
    wait(&mut app, &ctx);
    let session = app.session.as_ref().unwrap();
    assert!(!session.vault.is_writable());
    assert_eq!(session.vault.search("资料", false).count(), 1);
    app.lock(&ctx);
    assert!(
        Vault::open(
            &path,
            Credential::Password(&Password::new("中文cafe\u{301}".into()).unwrap()),
            OpenMode::ReadWrite
        )
        .is_ok()
    );
}

#[test]
fn close_during_create_discards_late_success_and_releases_lock() {
    let ctx = egui::Context::default();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("vault");
    let mut app = VaultApp::default();
    create(&mut app, &path, &ctx);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.join("keys.json").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    app.tick(&ctx, Instant::now(), false, true);
    wait(&mut app, &ctx);
    assert!(app.session.is_none() && app.close_when_done);
    assert!(
        Vault::open(
            &path,
            Credential::Password(&Password::new("中文café".into()).unwrap()),
            OpenMode::ReadWrite
        )
        .is_ok()
    );
}

#[test]
fn gui_confirmation_uses_shared_nfc_rules_and_clears_secrets_on_error() {
    let mut form = Form {
        create: true,
        phrase: Zeroizing::new("中文café".into()),
        confirmation: Zeroizing::new("中文cafe\u{301}".into()),
        ..Default::default()
    };
    assert!(matches!(form.request(), Ok(Request::Create(_, _))));
    form.phrase = Zeroizing::new("第一遍".into());
    form.confirmation = Zeroizing::new("不匹配".into());
    assert!(matches!(form.request(), Err(Error::InvalidInput)));
    assert!(form.phrase.is_empty() && form.confirmation.is_empty());
    form.create = false;
    form.recovery = true;
    form.phrase = RecoveryKey::generate().unwrap().encode().unwrap();
    assert!(matches!(form.request(), Ok(Request::Recover(_))));
    assert!(form.phrase.is_empty());
}
