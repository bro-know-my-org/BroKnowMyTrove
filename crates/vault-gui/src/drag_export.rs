use eframe::egui;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};
use vault_core::{Error, Id, Result, Vault, operation::Operation};
use vault_i18n::TextKey;

#[cfg(target_os = "linux")]
mod x11;

pub fn support(frame: &eframe::Frame) -> std::result::Result<(), TextKey> {
    #[cfg(target_os = "linux")]
    {
        if x11::window(frame).is_none() {
            return Err(TextKey::DragExportX11Required);
        }
        if !x11::local_display(None) {
            return Err(TextKey::DragExportX11Transport);
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = frame;
        cfg!(any(windows, target_os = "macos"))
            .then_some(())
            .ok_or(TextKey::DragExportUnavailable)
    }
}

pub struct Prepared {
    directory: tempfile::TempDir,
    path: PathBuf,
}

impl Prepared {
    fn new(vault: &Vault, id: Id) -> Result<Self> {
        let name = &vault.entry(id).ok_or(Error::InvalidInput)?.name;
        let directory = tempfile::Builder::new().prefix(".bkmt-drag-").tempdir()?;
        let path = directory.path().join(name);
        Ok(Self { directory, path })
    }
}

pub fn prepare(session: &mut crate::Session, id: Id, operation: &Operation) -> Result<()> {
    prepare_with(session, id, |vault, path| {
        vault.export_file(id, path, operation)
    })
}

fn prepare_with(
    session: &mut crate::Session,
    id: Id,
    export: impl FnOnce(&Vault, &std::path::Path) -> Result<()>,
) -> Result<()> {
    // Register ownership BEFORE writing any plaintext. Even ExportUncertain or
    // failed temporary-file deletion must leave a directory available for retry.
    session.drag_export = Some(Prepared::new(&session.vault, id)?);
    export(&session.vault, &session.drag_export.as_ref().unwrap().path)
}

/// Keep successful source files until lock: file managers may copy asynchronously
/// after reporting a successful drop. A canceled drag is removed on the next tick.
#[derive(Default)]
pub struct Exports {
    active: Option<Prepared>,
    retained: Vec<Prepared>,
    cleanup: Vec<Prepared>,
    preparing: Option<bool>,
    outcome: Arc<AtomicU8>,
    #[cfg(target_os = "linux")]
    native: Option<x11::Drag>,
}

impl Exports {
    pub fn busy(&self) -> bool {
        self.active.is_some()
    }

    pub fn begin_preparation(&mut self) {
        self.preparing = Some(true);
    }

    pub fn observe_gesture(&mut self, ctx: &egui::Context) {
        if let Some(held) = &mut self.preparing {
            *held &= ctx.input(|input| {
                input.focused
                    && input.pointer.primary_down()
                    && !input.pointer.button_released(egui::PointerButton::Primary)
            });
        }
    }

    pub fn needs_cleanup(&self) -> bool {
        !self.cleanup.is_empty()
    }

    pub fn discard(&mut self, prepared: Prepared) {
        self.cleanup.push(prepared);
    }

    pub fn lock(&mut self) {
        #[cfg(target_os = "linux")]
        {
            self.native = None;
        }
        self.preparing = None;
        self.outcome = Arc::default(); // Ignore callbacks from a canceled session.
        self.cleanup.extend(self.active.take());
        self.cleanup.append(&mut self.retained);
    }

    fn retry_cleanup(&mut self) {
        self.cleanup.retain(
            |export| match std::fs::remove_dir_all(export.directory.path()) {
                Ok(()) => false,
                Err(error) => error.kind() != std::io::ErrorKind::NotFound,
            },
        );
    }

    pub fn contains(&self, path: &std::path::Path) -> bool {
        self.active
            .iter()
            .chain(&self.retained)
            .chain(&self.cleanup)
            .any(|export| path.starts_with(export.directory.path()))
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> Option<TextKey> {
        #[cfg(target_os = "linux")]
        if let Some(native) = &self.native {
            native.poll();
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        let was_dirty = self.needs_cleanup();
        let outcome = self.outcome.swap(0, Ordering::SeqCst);
        if outcome != 0 {
            #[cfg(target_os = "linux")]
            {
                self.native = None;
            }
            // Native drag loops may consume the release without notifying egui.
            ctx.stop_dragging();
            ctx.input_mut(|input| input.pointer = egui::PointerState::default());
        }
        let message = match outcome {
            1 | 4 => {
                if let Some(export) = self.active.take() {
                    self.retained.push(export);
                }
                Some(if outcome == 1 {
                    TextKey::DragExportDropped
                } else {
                    TextKey::DragExportUnconfirmed
                })
            }
            2 => {
                self.cleanup.extend(self.active.take());
                Some(TextKey::DragExportCancelled)
            }
            3 => {
                self.cleanup.extend(self.active.take());
                Some(TextKey::DragExportUnavailable)
            }
            _ => None,
        };
        self.retry_cleanup();
        if self.needs_cleanup() {
            Some(TextKey::DragCleanupFailed)
        } else {
            message.or(was_dirty.then_some(TextKey::DragCleanupDone))
        }
    }

    pub fn start(
        &mut self,
        prepared: Prepared,
        frame: &eframe::Frame,
        ctx: &egui::Context,
    ) -> TextKey {
        self.observe_gesture(ctx);
        if self.preparing.take() != Some(true) {
            self.discard(prepared);
            self.poll(ctx);
            return if self.needs_cleanup() {
                TextKey::DragCleanupFailed
            } else {
                TextKey::DragExportCancelled
            };
        }
        self.outcome = Arc::default();
        #[cfg(target_os = "linux")]
        let result = {
            self.native =
                x11::Drag::start(frame, &prepared.path, self.outcome.clone(), ctx.clone()).ok();
            self.native.is_some()
        };
        #[cfg(not(target_os = "linux"))]
        let result = native_start(frame, &prepared.path, self.outcome.clone(), ctx.clone());
        if result {
            self.active = Some(prepared);
            TextKey::DragExportActive
        } else {
            self.discard(prepared);
            self.poll(ctx);
            if self.needs_cleanup() {
                TextKey::DragCleanupFailed
            } else {
                TextKey::DragExportUnavailable
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub fn input(&self, ctx: &egui::Context, raw: &egui::RawInput) {
        if let Some(native) = &self.native {
            native.input(ctx, raw);
        }
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn native_start(
    frame: &eframe::Frame,
    path: &std::path::Path,
    outcome: Arc<AtomicU8>,
    ctx: egui::Context,
) -> bool {
    // A valid 1x1 PNG. The system also supplies its regular file/copy cursor.
    const ICON: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5,
        1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    drag::start_drag(
        frame,
        drag::DragItem::Files(vec![path.to_owned()]),
        drag::Image::Raw(ICON.to_vec()),
        move |result, _| {
            outcome.store(
                match result {
                    drag::DragResult::Dropped => 1,
                    drag::DragResult::Cancel => 2,
                },
                Ordering::SeqCst,
            );
            ctx.request_repaint();
        },
        drag::Options::default(),
    )
    .is_ok()
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn native_start(
    _: &eframe::Frame,
    _: &std::path::Path,
    _: Arc<AtomicU8>,
    _: egui::Context,
) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_plaintext_authenticates_stays_private_and_is_removed_on_lock() {
        let ctx = egui::Context::default();
        let temp = tempfile::tempdir().unwrap();
        let mut app = crate::VaultApp::default();
        crate::tests::create(&mut app, &temp.path().join("vault"), &ctx);
        crate::tests::wait(&mut app, &ctx);
        let source = temp.path().join("中文.txt");
        std::fs::write(&source, b"secret plaintext").unwrap();
        app.start_action(
            crate::Action::Import(None, zeroize::Zeroizing::new("中文.txt".into()), source),
            &ctx,
        );
        crate::tests::wait(&mut app, &ctx);
        let id = app
            .session
            .as_ref()
            .unwrap()
            .vault
            .entries()
            .next()
            .unwrap()
            .id;
        app.start_action(crate::Action::PrepareDrag(id), &ctx);
        crate::tests::wait(&mut app, &ctx);
        let prepared = app.session.as_mut().unwrap().drag_export.take().unwrap();
        let path = prepared.path.clone();
        assert_eq!(std::fs::read(&path).unwrap(), b"secret plaintext");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
                0
            );
        }
        app.exports.active = Some(prepared);
        app.exports.outcome.store(1, Ordering::SeqCst);
        app.poll(&ctx);
        assert!(path.exists()); // OS acceptance need not mean copying has finished.
        assert!(app.exports.contains(&path));
        let failure = prepare_with(app.session.as_mut().unwrap(), id, |vault, destination| {
            vault.export_file(id, destination, &Operation::default())?;
            Err(Error::ExportUncertain)
        });
        assert!(matches!(failure, Err(Error::ExportUncertain)));
        let failed_path = app
            .session
            .as_ref()
            .unwrap()
            .drag_export
            .as_ref()
            .unwrap()
            .path
            .clone();
        assert!(failed_path.exists());
        app.lock(&ctx);
        assert!(!path.exists());
        assert!(!failed_path.exists());
    }

    #[test]
    fn cancellation_and_early_release_remove_staging_without_starting_native_drag() {
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("plain.txt");
        std::fs::write(&path, b"plaintext").unwrap();
        let mut exports = Exports::default();
        exports.start(
            Prepared {
                directory,
                path: path.clone(),
            },
            &frame,
            &ctx,
        );
        assert!(!path.exists());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("plain.txt");
        std::fs::write(&path, b"plaintext").unwrap();
        exports.active = Some(Prepared {
            directory,
            path: path.clone(),
        });
        exports.outcome.store(2, Ordering::SeqCst);
        assert!(matches!(
            exports.poll(&ctx),
            Some(TextKey::DragExportCancelled)
        ));
        assert!(!path.exists());
        assert!(!exports.busy());
    }

    #[test]
    fn release_then_repress_cannot_resurrect_preparing_drag() {
        let ctx = egui::Context::default();
        let mut exports = Exports::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::PointerButton {
                    pos: egui::pos2(10.0, 10.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |_| {},
        );
        output.textures_delta.clear();
        exports.begin_preparation();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![false, true]
                    .into_iter()
                    .map(|pressed| egui::Event::PointerButton {
                        pos: egui::pos2(10.0, 10.0),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    })
                    .collect(),
                ..Default::default()
            },
            |ui| exports.observe_gesture(ui.ctx()),
        );
        output.textures_delta.clear();
        assert!(ctx.input(|input| input.pointer.primary_down()));
        assert_eq!(exports.preparing, Some(false));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("plain.txt");
        std::fs::write(&path, b"plaintext").unwrap();
        exports.start(
            Prepared {
                directory,
                path: path.clone(),
            },
            &eframe::Frame::_new_kittest(),
            &ctx,
        );
        assert!(!path.exists());
        assert!(!exports.busy());
    }

    #[test]
    #[cfg(windows)]
    fn locked_export_cleanup_retries_after_windows_copy_handle_is_released() {
        use std::os::windows::fs::OpenOptionsExt;
        let ctx = egui::Context::default();
        let temp = tempfile::tempdir().unwrap();
        let mut app = crate::VaultApp::default();
        crate::tests::create(&mut app, &temp.path().join("vault"), &ctx);
        crate::tests::wait(&mut app, &ctx);
        let source = temp.path().join("source.txt");
        std::fs::write(&source, b"plaintext").unwrap();
        let session = app.session.as_mut().unwrap();
        let id = session
            .vault
            .import_file(None, "plain.txt", &source, &Operation::default())
            .unwrap();
        let mut held = None;
        let failure = prepare_with(session, id, |vault, path| {
            vault.export_file(id, path, &Operation::default())?;
            held = Some(
                std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(path)?,
            );
            Err(Error::ExportUncertain)
        });
        assert!(matches!(failure, Err(Error::ExportUncertain)));
        let path = session.drag_export.as_ref().unwrap().path.clone();
        app.lock(&ctx);
        app.poll(&ctx);
        assert!(app.exports.needs_cleanup());
        assert!(
            app.notice
                .render(vault_i18n::Locale::EnUs)
                .contains("cleanup")
        );
        assert!(path.exists());
        app.tick(&ctx, std::time::Instant::now(), true, true);
        assert!(app.close_when_done);
        drop(held);
        app.poll(&ctx);
        assert!(!app.exports.needs_cleanup());
        assert!(!path.exists());
    }
}
