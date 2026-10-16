use crate::picker::{PathInput, Target};
use crate::{Browser, actions::Action};
use eframe::egui;
use vault_core::{EntryKind, Id, Vault};
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
    ) -> Option<Action> {
        if let Some(action) = &self.confirmation {
            ui.heading("确认操作");
            ui.vertical(|ui| {
                ui.label(match action {
                    Action::Replace(..) => "确认用本地文件替换所选文件的内容？",
                    Action::Trash(_) => "确认移入加密回收站？源文件不受影响。",
                    _ => "将退役全部本地历史恢复点并回收对象；清空还会永久移除回收站条目。独立备份不受影响。",
                });
                if let Action::Replace(id, _) | Action::Trash(id) = action
                    && let Some(entry) = vault.entry(*id) { ui.label(&entry.name); }
                if let Action::Replace(_, source) = action { ui.label(source.display().to_string()); }
            });
            if ui.button("确认执行").clicked() {
                return self.confirmation.take();
            }
            if ui.button("取消").clicked() {
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
                    EntryKind::Directory => "文件夹".into(),
                    EntryKind::File { length, .. } => {
                        format!("文件 · {}", crate::theme::size(length))
                    }
                },
            );
            ui.separator();
        } else {
            crate::theme::caption(ui, "选择项目以查看详情和管理文件。");
        }
        ui.label("名称");
        ui.add(
            egui::TextEdit::singleline(&mut *self.name)
                .desired_width(f32::INFINITY)
                .margin(egui::Margin::symmetric(8, 5))
                .hint_text("文件或目录名称"),
        );
        if let Some(entry) = active
            && writable
            && ui.button("重命名").clicked()
        {
            action = Some(Action::Rename(entry.id, self.name.clone()));
        }
        egui::CollapsingHeader::new(if file.is_some() {
            "导入、导出与替换"
        } else {
            "导入与新建"
        })
        .id_salt(("transfer", selected.is_some()))
        .default_open(selected.is_none())
        .show(ui, |ui| {
            ui.label("本地文件路径");
            self.path.ui(ui);
            ui.horizontal(|ui| {
                if ui.button("选择源文件").clicked() {
                    *picker = Some(Target::Source);
                }
                if ui.button("导出位置").clicked() {
                    *picker = Some(Target::Destination);
                }
            });
            crate::theme::caption(
                ui,
                "导入 / 替换的源文件，或导出目标。导入保留源，导出拒绝覆盖。",
            );
            ui.horizontal_wrapped(|ui| {
                if clicked(ui, "新建目录", writable && !browser.trash) {
                    action = Some(Action::Mkdir(browser.parent, self.name.clone()));
                }
                if clicked(
                    ui,
                    "导入",
                    writable && !browser.trash && !self.path.is_empty(),
                ) {
                    action = Some(Action::Import(
                        browser.parent,
                        self.name.clone(),
                        self.path.path(),
                    ));
                }
                if clicked(ui, "导出", file.is_some() && !self.path.is_empty()) {
                    action = Some(Action::Export(file.unwrap().id, self.path.path()));
                }
                if clicked(
                    ui,
                    "替换内容",
                    writable && active.is_some() && file.is_some() && !self.path.is_empty(),
                ) {
                    action = Some(Action::Replace(file.unwrap().id, self.path.path()));
                }
            });
        });
        ui.separator();
        ui.collapsing("移动与回收", |ui| {
            egui::ComboBox::from_id_salt("move-parent")
                .width(ui.available_width())
                .truncate()
                .selected_text(
                    self.parent
                        .and_then(|id| vault.entry(id))
                        .map_or("根目录", |entry| &entry.name),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.parent, None, "根目录");
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
            if clicked(ui, "移动到目录", writable && active.is_some()) {
                action = Some(Action::Move(active.unwrap().id, self.parent));
            }
            if clicked(ui, "移入回收站", writable && active.is_some()) {
                action = Some(Action::Trash(active.unwrap().id));
            }
            if clicked(
                ui,
                "恢复所选",
                writable && selected.is_some_and(|entry| entry.is_trashed()),
            ) {
                action = Some(Action::Restore(selected.unwrap().id));
            }
        });
        ui.add_space(4.0);
        ui.collapsing("保险库维护", |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("完整内容检查").clicked() {
                    action = Some(Action::Check);
                }
                if ui.button("空间统计").clicked() {
                    action = Some(Action::Space);
                }
                if clicked(ui, "清空回收站", writable) {
                    action = Some(Action::EmptyTrash);
                }
                if clicked(ui, "回收空间", writable) {
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
