use eframe::egui;
use std::time::{Duration, Instant};
use vault_core::{
    CreateOptions, EntryKind, Error, Id, OpenMode, Result,
    crypto::{Password, RecoveryKey},
};
use vault_i18n::{Locale, TextKey};
use zeroize::Zeroizing;

mod actions;
mod files_ui;
mod picker;
mod session;
mod theme;
use actions::Action;
use picker::{NEW_VAULT_DIRECTORY, PathInput, Picker, Target};
use session::{Request, Session, Task};
use vault_i18n::{ErrorContext, Message};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 700.0])
            .with_min_inner_size([760.0, 520.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        vault_core::PRODUCT_NAME,
        options,
        Box::new(|cc| {
            let mut fonts = egui::FontDefinitions::default();
            let mut cjk = egui::FontData::from_static(include_bytes!(
                "../../../assets/fonts/NotoSansCJK-Regular.ttc"
            ));
            cjk.index = 2; // Noto Sans CJK SC in the unmodified, licensed TTC collection.
            fonts
                .font_data
                .insert("noto-cjk".into(), std::sync::Arc::new(cjk));
            for family in fonts.families.values_mut() {
                family.push("noto-cjk".into());
            }
            fonts.font_data.insert(
                "serif-ui".into(),
                std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
                    "../../../assets/fonts/NotoSerifUI.otf"
                ))),
            );
            fonts.families.insert(
                egui::FontFamily::Name("serif".into()),
                vec!["serif-ui".into(), "noto-cjk".into()],
            );
            cc.egui_ctx.set_fonts(fonts);
            theme::install(&cc.egui_ctx);
            Ok(Box::new(VaultApp::default()))
        }),
    )
}

#[derive(Default)]
struct Form {
    path: PathInput,
    phrase: Zeroizing<String>,
    confirmation: Zeroizing<String>,
    create: bool,
    recovery: bool,
    show_password: bool,
    read_only: bool,
}

impl Form {
    fn request(&mut self) -> Result<Request> {
        let mode = if self.read_only {
            OpenMode::ReadOnly
        } else {
            OpenMode::ReadWrite
        };
        let mut phrase = std::mem::take(&mut self.phrase);
        let mut confirmation = std::mem::take(&mut self.confirmation);
        self.show_password = false;
        if !self.create && self.recovery {
            return Ok(Request::Recover(RecoveryKey::decode(&phrase)?, mode));
        }
        let password = Password::new(std::mem::take(&mut *phrase))?;
        if self.create {
            if !password.matches(&Password::new(std::mem::take(&mut *confirmation))?) {
                return Err(Error::InvalidInput);
            }
            Ok(Request::Create(
                password,
                CreateOptions {
                    recovery_key: self.recovery,
                    ..Default::default()
                },
            ))
        } else {
            Ok(Request::Open(password, mode))
        }
    }
}

#[derive(Default)]
struct Browser {
    query: Zeroizing<String>,
    parent: Option<Id>,
    trash: bool,
    selected: Option<Id>,
}

#[derive(Default)]
struct VaultApp {
    form: Form,
    browser: Browser,
    editor: files_ui::Editor,
    session: Option<Session>,
    task: Option<Task>,
    picker: Option<Picker>,
    discard_result: bool,
    close_when_done: bool,
    locale: Locale,
    notice: Message,
    last_activity: Option<Instant>,
}

impl VaultApp {
    fn lock(&mut self, ctx: &egui::Context) {
        if let Some(task) = &self.task {
            task.operation.cancel();
            self.discard_result = true;
        }
        self.session = None;
        self.picker = None;
        self.browser = Browser::default();
        self.editor = files_ui::Editor::default();
        self.form.phrase = Zeroizing::default();
        self.form.confirmation = Zeroizing::default();
        self.form.show_password = false;
        // Also drop widget editing/undo state. No eframe persistence is enabled.
        theme::clear_widget_memory(ctx);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(selection) = self.picker.as_ref().and_then(Picker::poll) {
            let target = self.picker.take().unwrap().target;
            if let Some(path) = selection {
                match target {
                    Target::Vault => self.form.path.set(if self.form.create {
                        path.join(NEW_VAULT_DIRECTORY)
                    } else {
                        path
                    }),
                    Target::Source => {
                        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                            self.editor.name = Zeroizing::new(name.into());
                        }
                        self.editor.path.set(path);
                    }
                    Target::Destination => self.editor.path.set(path),
                }
            } else {
                self.notice = TextKey::PickerCancelled.into();
            }
        }
        if let Some(result) = self.task.as_ref().and_then(Task::poll) {
            self.task = None; // Join before accepting another operation.
            if self.discard_result {
                drop(result);
                self.notice = TextKey::StoppedLocked.into();
            } else {
                self.session = result.session;
                match result.result {
                    Ok(message) => {
                        self.form.create = false;
                        self.form.recovery = false;
                        self.notice = message;
                        self.last_activity = Some(Instant::now());
                    }
                    Err(error) => self.notice = Message::Error(ErrorContext::Operation, error),
                }
                if self.session.is_none() {
                    self.lock(ctx);
                }
            }
            self.discard_result = false;
            if self.close_when_done {
                self.lock(ctx);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn tick(&mut self, ctx: &egui::Context, time: Instant, active: bool, close: bool) {
        if active {
            self.last_activity = Some(time);
        }
        if (self.session.is_some() || self.task.is_some())
            && self
                .last_activity
                .is_some_and(|last| time.duration_since(last) >= Duration::from_secs(300))
        {
            self.lock(ctx);
            self.notice = TextKey::IdleLocked.into();
        }
        if close {
            self.close_when_done = self.task.is_some();
            if self.close_when_done {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            self.lock(ctx);
            if !self.close_when_done {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        self.poll(ctx);
    }

    fn login(&mut self, ui: &mut egui::Ui) {
        let locale = self.locale;
        ui.horizontal(|ui| {
            for (create, label) in [
                (false, locale.text(TextKey::OpenVault)),
                (true, locale.text(TextKey::NewVaultTab)),
            ] {
                if ui
                    .add_sized(
                        [112.0, 28.0],
                        egui::Button::selectable(self.form.create == create, label),
                    )
                    .clicked()
                    && self.form.create != create
                {
                    self.form.create = create;
                    self.form.recovery = false;
                    self.form.phrase = Zeroizing::default();
                    self.form.confirmation = Zeroizing::default();
                    self.form.show_password = false;
                    theme::clear_widget_memory(ui.ctx());
                }
            }
        });
        ui.add_space(8.0);
        ui.heading(if self.form.create {
            locale.text(TextKey::CreateVault)
        } else {
            locale.text(TextKey::UnlockVault)
        });
        theme::caption(
            ui,
            if self.form.create {
                locale.text(TextKey::CreateHint)
            } else {
                locale.text(TextKey::UnlockHint)
            },
        );
        ui.add_space(6.0);
        ui.label(locale.text(TextKey::VaultDirectory));
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2((ui.available_width() - 68.0).max(120.0), 28.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.form.path.ui(ui, locale),
            );
            if ui
                .button(locale.text(TextKey::Browse))
                .on_hover_text(locale.text(TextKey::BrowseHint))
                .clicked()
            {
                self.pick(Target::Vault, ui.ctx());
            }
        });
        if self.form.create {
            theme::caption(ui, locale.text(TextKey::DirectoryHint));
        }
        ui.label(if !self.form.create && self.form.recovery {
            locale.text(TextKey::RecoveryKey)
        } else {
            locale.text(TextKey::Password)
        });
        ui.add(
            egui::TextEdit::singleline(&mut *self.form.phrase)
                .char_limit(vault_core::crypto::PASSWORD_MAX)
                .desired_width(f32::INFINITY)
                .margin(egui::Margin::symmetric(8, 5))
                .hint_text(if self.form.create {
                    locale.text(TextKey::SetPassword)
                } else {
                    locale.text(TextKey::EnterPassword)
                })
                .password(!self.form.show_password),
        );
        if self.form.create {
            ui.label(locale.text(TextKey::ConfirmPassword));
            ui.add(
                egui::TextEdit::singleline(&mut *self.form.confirmation)
                    .char_limit(vault_core::crypto::PASSWORD_MAX)
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::symmetric(8, 5))
                    .password(!self.form.show_password),
            );
        }
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut self.form.show_password,
                locale.text(TextKey::ShowInput),
            );
            if !self.form.create {
                ui.checkbox(&mut self.form.read_only, locale.text(TextKey::OpenReadOnly));
            }
        });
        ui.checkbox(
            &mut self.form.recovery,
            if self.form.create {
                locale.text(TextKey::GenerateRecovery)
            } else {
                locale.text(TextKey::UnlockRecovery)
            },
        );
        ui.add_space(8.0);
        if ui
            .add_enabled(
                !self.form.path.is_empty(),
                theme::primary(if self.form.create {
                    locale.text(TextKey::CreateVault)
                } else {
                    locale.text(TextKey::UnlockVault)
                })
                .min_size(egui::vec2(ui.available_width(), 32.0)),
            )
            .clicked()
        {
            let result = self
                .form
                .request()
                .and_then(|request| Task::start(self.form.path.path(), request, ui.ctx().clone()));
            theme::clear_widget_memory(ui.ctx());
            match result {
                Ok(task) => {
                    self.task = Some(task);
                    self.notice = Message::Empty;
                }
                Err(error) => self.notice = Message::Error(ErrorContext::Input, error),
            }
        }
    }

    fn start_action(&mut self, action: Action, ctx: &egui::Context) {
        let Some(session) = self.session.take() else {
            return;
        };
        match Task::action(session, action, ctx.clone()) {
            Ok(task) => {
                self.task = Some(task);
                self.notice = Message::Empty;
                self.last_activity = Some(Instant::now());
            }
            Err(error) => {
                self.lock(ctx);
                self.notice = Message::Error(ErrorContext::Task, error);
            }
        }
    }

    fn pick(&mut self, target: Target, ctx: &egui::Context) {
        match Picker::start(target, ctx.clone()) {
            Ok(picker) => self.picker = Some(picker),
            Err(error) => self.notice = Message::Error(ErrorContext::Picker, error),
        }
    }

    fn browse(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let locale = self.locale;
        let session = self.session.as_mut().expect("unlocked session");
        if let Some(recovery) = &mut session.recovery {
            let mut saved = false;
            theme::card().show(ui, |ui| {
                ui.heading(locale.text(TextKey::SaveRecovery));
                ui.label(locale.text(TextKey::RecoveryWarning));
                let mut text = recovery.as_str();
                ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(8, 5)),
                );
                saved = ui
                    .add(theme::primary(locale.text(TextKey::RecoverySaved)))
                    .clicked();
            });
            if saved {
                session.recovery = None;
                theme::clear_widget_memory(ui.ctx());
            }
            return None;
        }
        let mut picker = None;
        let action = egui::Panel::right("file-actions")
            .default_size(286.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::SURFACE).inner_margin(16))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("actions-scroll")
                    .show(ui, |ui| {
                        ui.heading(locale.text(TextKey::FileDetails));
                        ui.add_space(8.0);
                        self.editor
                            .ui(ui, &session.vault, &self.browser, &mut picker, locale)
                    })
                    .inner
            })
            .inner;
        if ui.available_width() > 570.0 {
            egui::Panel::left("navigation")
                .exact_size(168.0)
                .resizable(false)
                .frame(egui::Frame::new().fill(theme::DEEP).inner_margin(12))
                .show(ui, |ui| {
                    ui.heading(locale.text(TextKey::Vault));
                    ui.add_space(8.0);
                    for (trash, label) in [
                        (false, locale.text(TextKey::AllFiles)),
                        (true, locale.text(TextKey::Trash)),
                    ] {
                        if ui
                            .add_sized(
                                [144.0, 30.0],
                                egui::Button::selectable(self.browser.trash == trash, ())
                                    .left_text(egui::RichText::new(label).color(theme::INK)),
                            )
                            .clicked()
                        {
                            self.browser = Browser {
                                trash,
                                ..Default::default()
                            };
                        }
                    }
                    ui.add_space(20.0);
                    theme::caption(
                        ui,
                        if session.vault.is_writable() {
                            locale.text(TextKey::WritableSession)
                        } else {
                            locale.text(TextKey::ReadOnlySession)
                        },
                    );
                    theme::caption(ui, locale.text(TextKey::LocalEncrypted));
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BACKGROUND).inner_margin(16))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(if self.browser.trash {
                        locale.text(TextKey::Trash)
                    } else {
                        locale.text(TextKey::MyFiles)
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .selectable_label(self.browser.trash, locale.text(TextKey::Trash))
                            .clicked()
                        {
                            self.browser = Browser {
                                trash: !self.browser.trash,
                                ..Default::default()
                            };
                        }
                    });
                });
                if !session.vault.issues().is_empty() {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        locale.text(TextKey::DamagedReferences),
                    );
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !self.browser.trash,
                            egui::Button::new(locale.text(TextKey::Root)),
                        )
                        .clicked()
                    {
                        self.browser.parent = None;
                        self.browser.query = Zeroizing::default();
                        self.browser.selected = None;
                    }
                    if ui
                        .add_enabled(
                            !self.browser.trash && self.browser.parent.is_some(),
                            egui::Button::new(locale.text(TextKey::Up)),
                        )
                        .clicked()
                    {
                        self.browser.parent = self
                            .browser
                            .parent
                            .and_then(|id| session.vault.entry(id))
                            .and_then(|entry| entry.parent);
                        self.browser.query = Zeroizing::default();
                        self.browser.selected = None;
                    }
                    ui.add(
                        egui::Label::new(
                            self.browser
                                .parent
                                .and_then(|id| session.vault.entry(id))
                                .map_or(locale.text(TextKey::AllFiles), |entry| {
                                    entry.name.as_str()
                                }),
                        )
                        .truncate(),
                    );
                });
                ui.add(
                    egui::TextEdit::singleline(&mut *self.browser.query)
                        .hint_text(locale.text(TextKey::SearchHint))
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(8, 5)),
                );
                let entries: Vec<_> = session
                    .vault
                    .search(&self.browser.query, self.browser.trash)
                    .filter(|entry| {
                        self.browser.trash
                            || !self.browser.query.is_empty()
                            || entry.parent == self.browser.parent
                    })
                    .collect();
                theme::caption(ui, &locale.items(entries.len()));
                ui.separator();
                if entries.is_empty() {
                    ui.add_space(32.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(if !self.browser.query.is_empty() {
                                locale.text(TextKey::NoMatches)
                            } else if self.browser.trash {
                                locale.text(TextKey::EmptyTrash)
                            } else {
                                locale.text(TextKey::NoFiles)
                            })
                            .text_style(egui::TextStyle::Heading),
                        );
                        theme::caption(
                            ui,
                            if self.browser.query.is_empty() && !self.browser.trash {
                                locale.text(TextKey::AddFilesHint)
                            } else {
                                locale.text(TextKey::SearchAdjustHint)
                            },
                        );
                    });
                }
                egui::ScrollArea::vertical()
                    .id_salt("files-scroll")
                    .show_rows(ui, 32.0, entries.len(), |ui, rows| {
                        for index in rows {
                            let entry = entries[index];
                            ui.horizontal(|ui| {
                                ui.set_min_height(32.0);
                                let (kind, size) = match entry.kind {
                                    EntryKind::Directory => {
                                        (locale.text(TextKey::Directory), "—".into())
                                    }
                                    EntryKind::File { length, .. } => {
                                        (locale.text(TextKey::File), theme::size(length))
                                    }
                                };
                                let width = (ui.available_width() - 104.0).max(80.0);
                                let response = ui.add_sized(
                                    [width, 26.0],
                                    egui::Button::selectable(
                                        self.browser.selected == Some(entry.id),
                                        (),
                                    )
                                    .left_text(&entry.name)
                                    .truncate(),
                                );
                                if response.clicked() {
                                    self.browser.selected = Some(entry.id);
                                    self.editor.name = Zeroizing::new(entry.name.clone());
                                }
                                if response.double_clicked()
                                    && !self.browser.trash
                                    && matches!(entry.kind, EntryKind::Directory)
                                {
                                    self.browser.parent = Some(entry.id);
                                    self.browser.selected = None;
                                    self.browser.query = Zeroizing::default();
                                }
                                response.on_hover_text(&entry.name);
                                if matches!(entry.kind, EntryKind::Directory) && !self.browser.trash
                                {
                                    if ui
                                        .small_button(locale.text(TextKey::OpenDirectory))
                                        .clicked()
                                    {
                                        self.browser.parent = Some(entry.id);
                                        self.browser.selected = None;
                                        self.browser.query = Zeroizing::default();
                                    }
                                } else {
                                    theme::caption(ui, &format!("{kind}  {size}"));
                                }
                            });
                        }
                    });
            });
        if let Some(target) = picker {
            self.pick(target, ui.ctx());
        }

        action
    }
}

impl eframe::App for VaultApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Hidden windows still run logic; their egui input time/events may be stale.
        let close = ctx.input(|input| {
            input.viewport().close_requested()
                || (input.modifiers.ctrl && input.key_pressed(egui::Key::Q))
        });
        self.tick(ctx, Instant::now(), false, close);
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let locale = self.locale;
        let ctx = ui.ctx().clone();
        if ctx.input(|input| !input.events.is_empty()) {
            self.last_activity = Some(Instant::now());
        }
        egui::Panel::top("app-header")
            .frame(
                egui::Frame::new()
                    .fill(theme::DEEP)
                    .inner_margin(egui::Margin::symmetric(16, 7)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::brand(ui);
                    egui::ComboBox::from_id_salt("language")
                        .selected_text(self.locale.code())
                        .width(88.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.locale, Locale::ZhCn, "简体中文");
                            ui.selectable_value(&mut self.locale, Locale::EnUs, "English");
                        });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(locale.text(TextKey::Exit))
                            .on_hover_text("Ctrl+Q")
                            .clicked()
                        {
                            self.tick(&ctx, Instant::now(), true, true);
                        }
                        if (self.session.is_some() || self.task.is_some() || self.picker.is_some())
                            && ui.button(locale.text(TextKey::LockCancel)).clicked()
                        {
                            self.lock(&ctx);
                        }
                        theme::caption(
                            ui,
                            if self.session.is_some() {
                                locale.text(TextKey::Unlocked)
                            } else {
                                locale.text(TextKey::Locked)
                            },
                        );
                    });
                });
            });
        egui::Panel::bottom("app-status")
            .frame(
                egui::Frame::new()
                    .fill(theme::DEEP)
                    .inner_margin(egui::Margin::symmetric(16, 5)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(locale.text(TextKey::Experimental))
                            .size(12.0)
                            .color(theme::MUTED),
                    )
                    .on_hover_text(locale.text(TextKey::ExperimentalWarning));
                    if self.picker.is_some() {
                        theme::caption(ui, locale.text(TextKey::WaitingPicker));
                    } else if !matches!(self.notice, Message::Empty) {
                        ui.label(self.notice.render(self.locale));
                    } else {
                        theme::caption(ui, locale.text(TextKey::AutoLock));
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BACKGROUND))
            .show(ui, |ui| {
                if let Some(task) = &self.task {
                    egui::Frame::new().inner_margin(24).show(ui, |ui| {
                        ui.heading(locale.text(TextKey::Working));
                        if let Some(progress) =
                            task.progress().filter(|progress| progress.total > 0)
                        {
                            ui.add(
                                egui::ProgressBar::new(
                                    progress.processed as f32 / progress.total as f32,
                                )
                                .animate(false)
                                .text(format!(
                                    "{} / {}",
                                    theme::size(progress.processed),
                                    theme::size(progress.total)
                                )),
                            );
                        }
                        ui.label(locale.text(TextKey::CancelWarning));
                    });
                } else if self.session.is_some() {
                    let action = ui
                        .add_enabled_ui(self.picker.is_none(), |ui| self.browse(ui))
                        .inner;
                    if let Some(action) = action {
                        self.start_action(action, &ctx);
                    }
                } else if !self.close_when_done {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add_space(32.0);
                        let width = ui.available_width();
                        ui.horizontal(|ui| {
                            ui.add_space(((width - 400.0) / 2.0).max(16.0));
                            ui.vertical(|ui| {
                                ui.set_width(400.0_f32.min((width - 32.0).max(240.0)));
                                ui.add_enabled_ui(self.picker.is_none(), |ui| self.login(ui));
                            });
                        });
                        ui.add_space(24.0);
                    });
                }
            });
    }
}

#[cfg(test)]
mod tests;
