use eframe::egui;
use std::{path::PathBuf, sync::mpsc, thread};
use vault_core::Result;
use vault_i18n::{Locale, TextKey};
use zeroize::Zeroizing;

/// A filesystem component, independent of the current display language.
pub const NEW_VAULT_DIRECTORY: &str = "新保险库";

/// Preserve native paths even when their display text is not lossless UTF-8.
#[derive(Default)]
pub struct PathInput {
    text: Zeroizing<String>,
    raw: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locking_discards_queued_picker_result() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut app = crate::VaultApp {
            picker: Some(Picker {
                target: Target::Source,
                receiver,
            }),
            ..Default::default()
        };
        sender.send(Some("private-source".into())).unwrap();
        let ctx = egui::Context::default();
        app.lock(&ctx);
        app.poll(&ctx);
        assert!(app.editor.path.is_empty());
        assert!(sender.send(None).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn native_path_preserves_non_utf8_bytes() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let original = PathBuf::from(OsString::from_vec(vec![b'/', b'x', 255]));
        let mut input = PathInput::default();
        input.set(original.clone());
        assert_eq!(input.path(), original);
    }

    #[test]
    fn language_changes_while_picking_do_not_change_the_destination() {
        for (before, after) in [(Locale::ZhCn, Locale::EnUs), (Locale::EnUs, Locale::ZhCn)] {
            for create in [false, true] {
                let (sender, receiver) = mpsc::sync_channel(1);
                let mut app = crate::VaultApp {
                    locale: before,
                    form: crate::Form {
                        create,
                        ..Default::default()
                    },
                    picker: Some(Picker {
                        target: Target::Vault,
                        receiver,
                    }),
                    ..Default::default()
                };
                app.locale = after;
                let parent = PathBuf::from("selected-parent");
                sender.send(Some(parent.clone())).unwrap();
                app.poll(&egui::Context::default());
                assert_eq!(
                    app.form.path.path(),
                    if create {
                        parent.join(NEW_VAULT_DIRECTORY)
                    } else {
                        parent
                    }
                );
                assert!(
                    after
                        .text(TextKey::BrowseHint)
                        .contains(NEW_VAULT_DIRECTORY)
                );
                assert!(
                    after
                        .text(TextKey::DirectoryHint)
                        .contains(NEW_VAULT_DIRECTORY)
                );
            }
        }
    }
}

impl PathInput {
    pub fn ui(&mut self, ui: &mut egui::Ui, locale: Locale) {
        if ui
            .add(
                egui::TextEdit::singleline(&mut *self.text)
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::symmetric(10, 8))
                    .hint_text(locale.text(TextKey::PathHint)),
            )
            .changed()
        {
            self.raw = None;
        }
    }
    pub fn set(&mut self, path: PathBuf) {
        self.text = Zeroizing::new(path.to_string_lossy().into_owned());
        self.raw = Some(path);
    }
    pub fn path(&self) -> PathBuf {
        self.raw
            .clone()
            .unwrap_or_else(|| self.text.as_str().into())
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

#[derive(Clone, Copy)]
pub enum Target {
    Vault,
    Source,
    Destination,
}

pub struct Picker {
    pub target: Target,
    receiver: mpsc::Receiver<Option<PathBuf>>,
}

impl Picker {
    pub fn start(target: Target, ctx: egui::Context) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(1);
        // This worker never owns a Vault, credentials, or permission to run an action.
        // Dropping the receiver on lock discards even a late selection.
        thread::Builder::new().spawn(move || {
            let dialog = rfd::FileDialog::new();
            let selection = match target {
                Target::Vault => dialog.pick_folder(),
                Target::Source => dialog.pick_file(),
                Target::Destination => dialog.save_file(),
            };
            let _ = sender.send(selection);
            ctx.request_repaint();
        })?;
        Ok(Self { target, receiver })
    }
    pub fn poll(&self) -> Option<Option<PathBuf>> {
        match self.receiver.try_recv() {
            Ok(path) => Some(path),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(None),
        }
    }
}
