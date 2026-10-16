use crate::picker::{PathInput, Target};
use crate::{Browser, actions::Action};
use eframe::egui;
use vault_core::{EntryKind, Id, Vault};
use vault_i18n::{Locale, TextKey};
use zeroize::Zeroizing;

#[derive(Default)]
pub struct Editor {
    pub name: Zeroizing<String>,
    pub path: PathInput,
    parent: Option<Id>,
    confirmation: Option<Action>,
}

impl Editor {
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        vault: &Vault,
        browser: &Browser,
        picker: &mut Option<Target>,
        locale: Locale,
    ) -> Option<Action> {
        if let Some(action) = &self.confirmation {
            ui.heading(locale.text(TextKey::ConfirmAction));
            ui.vertical(|ui| {
                ui.label(match action {
                    Action::Replace(..) => locale.text(TextKey::ConfirmReplace),
                    Action::Trash(_) => locale.text(TextKey::ConfirmTrash),
                    _ => locale.text(TextKey::ConfirmCleanup),
                });
                if let Action::Replace(id, _) | Action::Trash(id) = action
                    && let Some(entry) = vault.entry(*id)
                {
                    ui.label(&entry.name);
                }
                if let Action::Replace(_, source) = action {
                    ui.label(source.display().to_string());
                }
            });
            if ui.button(locale.text(TextKey::Execute)).clicked() {
                return self.confirmation.take();
            }
            if ui.button(locale.text(TextKey::Cancel)).clicked() {
                self.confirmation = None;
            }
            return None;
        }
        if self.parent.is_some_and(|id| vault.entry(id).is_none()) {
            self.parent = None;
        }
        let selected = browser.selected.and_then(|id| vault.entry(id));
        let active = selected.filter(|entry| !entry.is_trashed());
        let file = selected.filter(|entry| matches!(entry.kind, EntryKind::File { .. }));
        let writable = vault.is_writable();
        let mut action = None;
        if let Some(entry) = selected {
            ui.add(egui::Label::new(egui::RichText::new(&entry.name).size(16.0)).wrap());
            crate::theme::caption(
                ui,
                &match entry.kind {
                    EntryKind::Directory => locale.text(TextKey::Folder).into(),
                    EntryKind::File { length, .. } => locale.file_size(&crate::theme::size(length)),
                },
            );
            ui.separator();
        } else {
            crate::theme::caption(ui, locale.text(TextKey::SelectItemHint));
        }
        ui.label(locale.text(TextKey::Name));
        ui.add(
            egui::TextEdit::singleline(&mut *self.name)
                .desired_width(f32::INFINITY)
                .margin(egui::Margin::symmetric(8, 5))
                .hint_text(locale.text(TextKey::NameHint)),
        );
        if let Some(entry) = active
            && writable
            && ui.button(locale.text(TextKey::Rename)).clicked()
        {
            action = Some(Action::Rename(entry.id, self.name.clone()));
        }
        egui::CollapsingHeader::new(if file.is_some() {
            locale.text(TextKey::Transfer)
        } else {
            locale.text(TextKey::ImportCreate)
        })
        .id_salt(("transfer", selected.is_some()))
        .default_open(selected.is_none())
        .show(ui, |ui| {
            ui.label(locale.text(TextKey::LocalPath));
            self.path.ui(ui, locale);
            ui.horizontal(|ui| {
                if ui.button(locale.text(TextKey::SelectSource)).clicked() {
                    *picker = Some(Target::Source);
                }
                if ui.button(locale.text(TextKey::ExportLocation)).clicked() {
                    *picker = Some(Target::Destination);
                }
            });
            crate::theme::caption(ui, locale.text(TextKey::TransferHint));
            ui.horizontal_wrapped(|ui| {
                if clicked(
                    ui,
                    locale.text(TextKey::NewDirectory),
                    writable && !browser.trash,
                ) {
                    action = Some(Action::Mkdir(browser.parent, self.name.clone()));
                }
                if clicked(
                    ui,
                    locale.text(TextKey::Import),
                    writable && !browser.trash && !self.path.is_empty(),
                ) {
                    action = Some(Action::Import(
                        browser.parent,
                        self.name.clone(),
                        self.path.path(),
                    ));
                }
                if clicked(
                    ui,
                    locale.text(TextKey::Export),
                    file.is_some() && !self.path.is_empty(),
                ) {
                    action = Some(Action::Export(file.unwrap().id, self.path.path()));
                }
                if clicked(
                    ui,
                    locale.text(TextKey::Replace),
                    writable && active.is_some() && file.is_some() && !self.path.is_empty(),
                ) {
                    action = Some(Action::Replace(file.unwrap().id, self.path.path()));
                }
            });
        });
        ui.separator();
        egui::CollapsingHeader::new(locale.text(TextKey::MoveTrash))
            .id_salt("move-trash")
            .show(ui, |ui| {
                egui::ComboBox::from_id_salt("move-parent")
                    .width(ui.available_width())
                    .truncate()
                    .selected_text(
                        self.parent
                            .and_then(|id| vault.entry(id))
                            .map_or(locale.text(TextKey::Root), |entry| &entry.name),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.parent, None, locale.text(TextKey::Root));
                        for entry in vault.entries().filter(|entry| {
                            !entry.is_trashed() && matches!(entry.kind, EntryKind::Directory)
                        }) {
                            ui.selectable_value(
                                &mut self.parent,
                                Some(entry.id),
                                directory_path(vault, entry.id).as_str(),
                            );
                        }
                    });
                if clicked(
                    ui,
                    locale.text(TextKey::MoveTo),
                    writable && active.is_some(),
                ) {
                    action = Some(Action::Move(active.unwrap().id, self.parent));
                }
                if clicked(
                    ui,
                    locale.text(TextKey::MoveToTrash),
                    writable && active.is_some(),
                ) {
                    action = Some(Action::Trash(active.unwrap().id));
                }
                if clicked(
                    ui,
                    locale.text(TextKey::Restore),
                    writable && selected.is_some_and(|entry| entry.is_trashed()),
                ) {
                    action = Some(Action::Restore(selected.unwrap().id));
                }
            });
        ui.add_space(4.0);
        egui::CollapsingHeader::new(locale.text(TextKey::Maintenance))
            .id_salt("maintenance")
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button(locale.text(TextKey::CheckContent)).clicked() {
                        action = Some(Action::Check);
                    }
                    if ui.button(locale.text(TextKey::Space)).clicked() {
                        action = Some(Action::Space);
                    }
                    if clicked(ui, locale.text(TextKey::ClearTrash), writable) {
                        action = Some(Action::EmptyTrash);
                    }
                    if clicked(ui, locale.text(TextKey::Collect), writable) {
                        action = Some(Action::Collect);
                    }
                });
            });
        if action.as_ref().is_some_and(Action::needs_confirmation) {
            self.confirmation = action;
            None
        } else {
            action
        }
    }
}

fn directory_path(vault: &Vault, id: Id) -> Zeroizing<String> {
    let mut parts = Vec::new();
    let mut parent = Some(id);
    while let Some(entry) = parent.and_then(|id| vault.entry(id)) {
        parts.push(entry.name.as_str());
        parent = entry.parent;
    }
    parts.reverse();
    Zeroizing::new(parts.join("/"))
}

fn clicked(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    ui.add_enabled(enabled, egui::Button::new(label)).clicked()
}
