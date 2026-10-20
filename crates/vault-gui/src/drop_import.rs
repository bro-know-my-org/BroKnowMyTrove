use crate::{VaultApp, actions::Action};
use eframe::egui;
use std::{collections::VecDeque, path::PathBuf};
use vault_core::{EntryKind, Error, Id, Result, Vault, operation::Operation};
use vault_i18n::{Message, TextKey};

#[derive(Clone, Copy)]
pub enum Choice {
    Skip,
    Rename,
    Replace,
}

/// The worker returns the batch with its session whenever a decision is needed.
/// No thread waits for UI input and no conflict silently replaces existing data.
pub struct Batch {
    parent: Option<Id>,
    sources: VecDeque<PathBuf>,
    total: usize,
    completed: usize,
    skipped: usize,
    validated: bool,
    choice: Option<Choice>,
    policy: Option<Choice>,
    pub(super) apply_all: bool,
    pub(super) can_replace: bool,
}

impl Batch {
    pub fn new(parent: Option<Id>, sources: Vec<PathBuf>) -> Self {
        Self {
            parent,
            total: sources.len(),
            sources: sources.into(),
            completed: 0,
            skipped: 0,
            validated: false,
            choice: None,
            policy: None,
            apply_all: false,
            can_replace: false,
        }
    }

    /// `false` means paused at a conflict; all earlier commits remain durable.
    pub fn run(&mut self, vault: &mut Vault, operation: &Operation) -> Result<bool> {
        if !self.validated {
            if self.total == 0 || self.total > 1000 {
                return Err(Error::InvalidInput);
            }
            for source in &self.sources {
                operation.check_cancelled()?;
                if !source.is_absolute()
                    || !source.symlink_metadata()?.is_file()
                    || source.file_name().and_then(|name| name.to_str()).is_none()
                {
                    return Err(Error::InvalidInput);
                }
            }
            self.validated = true;
        }
        while let Some(source) = self.sources.front() {
            operation.check_cancelled()?;
            let name = source.file_name().and_then(|name| name.to_str()).unwrap();
            if let Some(entry) = vault.child_named(self.parent, name) {
                let id = entry.id;
                self.can_replace = matches!(entry.kind, EntryKind::File { .. });
                let Some(choice) = self.choice.take().or(self.policy) else {
                    return Ok(false);
                };
                match choice {
                    Choice::Skip => {
                        self.skipped += 1;
                        self.sources.pop_front();
                        continue;
                    }
                    Choice::Rename => {
                        let name =
                            zeroize::Zeroizing::new(vault.available_name(self.parent, name)?);
                        vault.import_file(self.parent, &name, source, operation)?;
                    }
                    Choice::Replace if self.can_replace => {
                        vault.replace_file(id, source, operation)?
                    }
                    Choice::Replace => return Ok(false), // A batch policy cannot replace a folder.
                }
            } else {
                vault.import_file(self.parent, name, source, operation)?;
            }
            self.completed += 1;
            self.sources.pop_front();
        }
        Ok(true)
    }

    pub fn summary(&self, error: Option<Error>) -> Message {
        Message::Imported(self.completed, self.total, self.skipped, error)
    }
}

impl VaultApp {
    pub(super) fn resolve_drop(&mut self, choice: Option<Choice>, ctx: &egui::Context) {
        let Some(mut batch) = self
            .session
            .as_mut()
            .and_then(|session| session.import_drop.take())
        else {
            return;
        };
        if let Some(choice) = choice {
            batch.choice = Some(choice);
            batch.policy = batch.apply_all.then_some(choice);
            self.start_action(Action::ImportDrop(batch), ctx);
        } else {
            self.notice = batch.summary(Some(Error::Cancelled));
        }
    }

    pub fn drop_conflict_ui(&mut self, ctx: &egui::Context) {
        let Some(batch) = self
            .session
            .as_mut()
            .and_then(|session| session.import_drop.as_mut())
        else {
            return;
        };
        let locale = self.locale;
        let mut choice = None;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("drop-name-conflict")).show(ctx, |ui| {
            ui.set_width(440.0_f32.min((ctx.content_rect().width() - 48.0).max(240.0)));
            ui.heading(locale.text(TextKey::DropConflict));
            ui.label(batch.sources.front().unwrap().display().to_string());
            ui.label(locale.text(TextKey::DropConflictHint));
            if !batch.can_replace {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    locale.text(TextKey::DropConflictDirectory),
                );
            }
            ui.checkbox(&mut batch.apply_all, locale.text(TextKey::DropApplyAll));
            ui.horizontal_wrapped(|ui| {
                for (value, key) in [
                    (Choice::Skip, TextKey::DropSkip),
                    (Choice::Rename, TextKey::DropRename),
                    (Choice::Replace, TextKey::DropReplace),
                ] {
                    if ui
                        .add_enabled(
                            !matches!(value, Choice::Replace) || batch.can_replace,
                            egui::Button::new(locale.text(key)),
                        )
                        .clicked()
                    {
                        choice = Some(value);
                    }
                }
            });
            cancel = ui.button(locale.text(TextKey::DropCancel)).clicked();
        });
        if choice.is_some() || cancel || response.should_close() {
            self.resolve_drop(choice, ctx);
        }
    }
}
