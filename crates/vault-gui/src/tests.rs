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
            Request::Open(Password::new("错误".into()).unwrap(), OpenMode::ReadOnly),
            ctx.clone(),
        )
        .unwrap(),
    );
    wait(&mut app, &ctx);
    assert!(app.session.is_none());
    assert!(
        app.notice
            .render(Locale::EnUs)
            .contains("authentication failed")
    );
    app.task = Some(
        Task::start(
            path.clone(),
            Request::Recover(recovery, OpenMode::ReadOnly),
            ctx.clone(),
        )
        .unwrap(),
    );
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
    assert!(matches!(form.request(), Ok(Request::Recover(_, _))));
    assert!(form.phrase.is_empty());
}

#[test]
fn gui_backup_and_restore_without_original_keeps_recovery_locked() {
    let ctx = egui::Context::default();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let backup = temp.path().join("backup");
    let restored = temp.path().join("restored");
    let mut app = VaultApp::default();
    create(&mut app, &source, &ctx);
    wait(&mut app, &ctx);
    let recovery =
        RecoveryKey::decode(app.session.as_ref().unwrap().recovery.as_ref().unwrap()).unwrap();
    act(
        &mut app,
        &ctx,
        Action::Mkdir(None, Zeroizing::new("恢复目录".into())),
    );
    let notice = act(
        &mut app,
        &ctx,
        Action::Backup(backup.clone(), BackupVerification::Full),
    );
    assert!(notice.contains("完整内容认证"));
    assert!(app.session.as_ref().unwrap().vault.is_writable());
    app.lock(&ctx);
    std::fs::remove_dir_all(source).unwrap();
    app.form.restore = true;
    app.form.destination.set(restored.clone());
    app.task = Some(
        Task::start(
            backup,
            Request::RestoreRecovery(recovery, restored.clone(), BackupVerification::Full),
            ctx.clone(),
        )
        .unwrap(),
    );
    wait(&mut app, &ctx);
    assert!(
        app.session.is_none(),
        "restore must not silently unlock a vault"
    );
    assert!(!app.form.restore);
    assert_eq!(app.form.path.path(), restored);
    assert!(app.form.destination.is_empty());
    assert!(matches!(app.notice, Message::Backup(_)));
    let vault = Vault::open(
        &restored,
        Credential::Password(&Password::new("中文café".into()).unwrap()),
        OpenMode::ReadWrite,
    )
    .unwrap();
    assert_eq!(vault.search("恢复目录", false).count(), 1);
}

#[test]
fn gui_restore_form_clears_credential_and_defaults_to_full() {
    let mut form = Form {
        restore: true,
        phrase: Zeroizing::new("测试口令".into()),
        ..Default::default()
    };
    form.destination.set("new-directory".into());
    assert!(matches!(
        form.request(),
        Ok(Request::RestorePassword(_, _, BackupVerification::Full))
    ));
    assert!(form.phrase.is_empty());
    form.recovery = true;
    form.phrase = Zeroizing::new("invalid recovery".into());
    assert!(form.request().is_err());
    assert!(form.phrase.is_empty());
}

fn act(app: &mut VaultApp, ctx: &egui::Context, action: Action) -> String {
    app.start_action(action, ctx);
    wait(app, ctx);
    assert!(
        app.session.is_some(),
        "an action must return its session even on error"
    );
    app.notice.render(app.locale)
}

#[test]
fn gui_credentials_clear_inputs_and_require_known_password_before_rotation() {
    use crate::credentials_ui::Credentials;
    let mut fields = Credentials {
        current: Zeroizing::new("old".into()),
        new: Zeroizing::new("中文café".into()),
        confirmation: Zeroizing::new("中文cafe\u{301}".into()),
        ..Default::default()
    };
    assert!(
        fields
            .prepare(TextKey::ChangePassword)
            .unwrap()
            .needs_confirmation()
    );
    assert!(fields.current.is_empty() && fields.new.is_empty() && fields.confirmation.is_empty());
    fields.new = Zeroizing::new("new".into());
    fields.confirmation = Zeroizing::new("mismatch".into());
    assert!(fields.prepare(TextKey::ChangePassword).is_err());
    assert!(fields.new.is_empty() && fields.confirmation.is_empty());

    let ctx = egui::Context::default();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("vault");
    let mut app = VaultApp::default();
    create(&mut app, &path, &ctx);
    wait(&mut app, &ctx);
    let old_key =
        RecoveryKey::decode(app.session.as_ref().unwrap().recovery.as_ref().unwrap()).unwrap();
    let header = std::fs::read(path.join("keys.json")).unwrap();
    assert!(
        act(
            &mut app,
            &ctx,
            Action::RotateRecovery(Password::new("wrong".into()).unwrap())
        )
        .contains("认证失败")
    );
    assert_eq!(std::fs::read(path.join("keys.json")).unwrap(), header);
    act(
        &mut app,
        &ctx,
        Action::ChangePassword(Password::new("new password".into()).unwrap()),
    );
    app.lock(&ctx);
    app.task = Some(
        Task::start(
            path.clone(),
            Request::Recover(old_key, OpenMode::ReadWrite),
            ctx.clone(),
        )
        .unwrap(),
    );
    wait(&mut app, &ctx);
    assert!(app.session.is_some());
    act(
        &mut app,
        &ctx,
        Action::RotateRecovery(Password::new("new password".into()).unwrap()),
    );
    let new_key =
        RecoveryKey::decode(app.session.as_ref().unwrap().recovery.as_ref().unwrap()).unwrap();
    app.lock(&ctx);
    app.task = Some(
        Task::start(
            path.clone(),
            Request::Recover(new_key, OpenMode::ReadWrite),
            ctx.clone(),
        )
        .unwrap(),
    );
    wait(&mut app, &ctx);
    act(
        &mut app,
        &ctx,
        Action::RemoveRecovery(Password::new("new password".into()).unwrap()),
    );
    assert!(app.session.as_ref().unwrap().recovery.is_none());
    app.editor.credentials.current = Zeroizing::new("discard me".into());
    app.lock(&ctx);
    assert!(app.editor.credentials.current.is_empty());
    let vault = Vault::open(
        &path,
        Credential::Password(&Password::new("new password".into()).unwrap()),
        OpenMode::ReadWrite,
    )
    .unwrap();
    assert!(vault.is_writable());
}

fn entry_id(app: &VaultApp, name: &str) -> Id {
    app.session
        .as_ref()
        .unwrap()
        .vault
        .entries()
        .find(|entry| entry.name == name)
        .unwrap()
        .id
}

#[test]
fn gui_file_jobs_roundtrip_preserve_source_and_session_on_failure() {
    let ctx = egui::Context::default();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("vault");
    let mut app = VaultApp::default();
    create(&mut app, &path, &ctx);
    wait(&mut app, &ctx);
    let source = temp.path().join("source");
    std::fs::write(&source, b"initial content").unwrap();
    act(
        &mut app,
        &ctx,
        Action::Mkdir(None, Zeroizing::new("目录".into())),
    );
    let directory = entry_id(&app, "目录");
    act(
        &mut app,
        &ctx,
        Action::Import(
            Some(directory),
            Zeroizing::new("资料".into()),
            source.clone(),
        ),
    );
    let file = entry_id(&app, "资料");
    let destination = temp.path().join("exported");
    act(&mut app, &ctx, Action::Export(file, destination.clone()));
    assert_eq!(std::fs::read(&destination).unwrap(), b"initial content");
    std::fs::write(&source, b"replacement").unwrap();
    assert!(Action::Replace(file, source.clone()).needs_confirmation());
    act(&mut app, &ctx, Action::Replace(file, source.clone()));
    assert!(act(&mut app, &ctx, Action::Export(file, destination.clone())).contains("冲突"));
    assert_eq!(std::fs::read(&destination).unwrap(), b"initial content");
    act(
        &mut app,
        &ctx,
        Action::Rename(file, Zeroizing::new("更名".into())),
    );
    act(&mut app, &ctx, Action::Move(file, None));
    act(&mut app, &ctx, Action::Trash(file));
    act(&mut app, &ctx, Action::Restore(file));
    assert!(act(&mut app, &ctx, Action::Check).contains("失败 0"));
    assert!(act(&mut app, &ctx, Action::Space).contains("活动"));
    act(&mut app, &ctx, Action::Trash(file));
    act(&mut app, &ctx, Action::EmptyTrash);
    act(&mut app, &ctx, Action::Collect);
    assert!(app.session.as_ref().unwrap().vault.entry(file).is_none());
    assert_eq!(std::fs::read(&source).unwrap(), b"replacement");
    app.lock(&ctx);
    app.task = Some(
        Task::start(
            path,
            Request::Open(
                Password::new("中文café".into()).unwrap(),
                OpenMode::ReadOnly,
            ),
            ctx.clone(),
        )
        .unwrap(),
    );
    wait(&mut app, &ctx);
    assert!(
        act(
            &mut app,
            &ctx,
            Action::Mkdir(None, Zeroizing::new("拒绝写入".into()))
        )
        .contains("禁止写入")
    );
    assert_eq!(app.session.as_ref().unwrap().vault.entries().count(), 1);
}

fn login_frame(ctx: &egui::Context, app: &mut VaultApp, events: Vec<egui::Event>) {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| app.login(ui));
        },
    );
    output.textures_delta.clear(); // This headless UI check does not upload textures.
}

fn click_login_tab(ctx: &egui::Context, app: &mut VaultApp, x: f32) {
    let pos = egui::pos2(x, 20.0);
    for pressed in [true, false] {
        login_frame(
            ctx,
            app,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}

#[test]
fn login_tabs_only_clear_sensitive_input_when_switching_modes() {
    let ctx = egui::Context::default();
    let mut app = VaultApp::default();
    app.form.create = true;
    app.form.phrase = Zeroizing::new("test password".into());
    app.form.confirmation = Zeroizing::new("test confirmation".into());
    app.form.recovery = true;
    app.form.show_password = true;
    login_frame(&ctx, &mut app, vec![]);
    let editing_state = egui::Id::new("editing-state");
    ctx.data_mut(|data| data.insert_temp(editing_state, 42_u32));

    click_login_tab(&ctx, &mut app, 180.0);
    assert!(app.form.create);
    assert_eq!(app.form.phrase.as_str(), "test password");
    assert_eq!(app.form.confirmation.as_str(), "test confirmation");
    assert!(app.form.recovery && app.form.show_password);
    assert_eq!(
        ctx.data(|data| data.get_temp::<u32>(editing_state)),
        Some(42)
    );

    click_login_tab(&ctx, &mut app, 50.0);
    assert!(!app.form.create);
    assert!(app.form.phrase.is_empty() && app.form.confirmation.is_empty());
    assert!(!app.form.recovery && !app.form.show_password);
    assert_eq!(ctx.data(|data| data.get_temp::<u32>(editing_state)), None);
}

#[test]
fn language_switch_renders_existing_job_results_and_keeps_vault_names() {
    let ctx = egui::Context::default();
    let temp = tempfile::tempdir().unwrap();
    let mut app = VaultApp::default();
    create(&mut app, &temp.path().join("vault"), &ctx);
    // Change language while the worker is running, before receiving its result.
    app.locale = Locale::EnUs;
    wait(&mut app, &ctx);
    act(
        &mut app,
        &ctx,
        Action::Mkdir(None, Zeroizing::new("资料".into())),
    );
    assert_eq!(app.notice.render(app.locale), "Operation completed.");
    app.start_action(Action::Space, &ctx);
    app.locale = Locale::ZhCn;
    wait(&mut app, &ctx);
    assert!(app.notice.render(app.locale).starts_with("活动"));
    app.locale = Locale::EnUs;
    assert!(app.notice.render(app.locale).starts_with("Active:"));
    let vault = &app.session.as_ref().unwrap().vault;
    assert_eq!(vault.entries().next().unwrap().name, "资料");
    app.notice = Message::Error(ErrorContext::Operation, Error::Authentication);
    assert!(
        app.notice
            .render(app.locale)
            .starts_with("authentication failed")
    );
    app.locale = Locale::ZhCn;
    assert!(app.notice.render(app.locale).starts_with("认证失败"));
    app.locale = Locale::EnUs;
    app.lock(&ctx);
    assert_eq!(app.locale, Locale::EnUs);
    assert!(app.form.phrase.is_empty());
    login_frame(&ctx, &mut app, vec![]);
}
