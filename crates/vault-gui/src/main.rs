use eframe::egui;
use std::time::{Duration, Instant};
use vault_core::{
    CreateOptions, EntryKind, Error, Id, OpenMode, Result,
    crypto::{Password, RecoveryKey},
};
use zeroize::Zeroizing;

mod actions;
mod files_ui;
mod picker;
mod session;
mod theme;
use actions::Action;
use picker::{PathInput, Picker, Target};
use session::{Request, Session, Task};

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
    notice: String,
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
                        path.join("新保险库")
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
                self.notice = "选择已取消或系统选择器不可用；可手填路径。".into();
            }
        }
        if let Some(result) = self.task.as_ref().and_then(Task::poll) {
            self.task = None; // Join before accepting another operation.
            if self.discard_result {
                drop(result);
                self.notice = "已停止并锁定；操作可能已提交，请重新打开核查。".into();
            } else {
                self.session = result.session;
                match result.result {
                    Ok(message) => {
                        self.form.create = false;
                        self.form.recovery = false;
                        self.notice = message;
                        self.last_activity = Some(Instant::now());
                    }
                    Err(error) => {
                        self.notice =
                            format!("{error}；已完成的提交不会回滚，结果不确定时请锁定并重新检查。")
                    }
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
            self.notice = "空闲超时，已锁定。".into();
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
        ui.horizontal(|ui| {
            for (create, label) in [(false, "打开保险库"), (true, "创建新保险库")] {
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
            "创建保险库"
        } else {
            "解锁保险库"
        });
        theme::caption(
            ui,
            if self.form.create {
                "选择一个新目录，设置你的保险库密码。"
            } else {
                "选择保险库，解锁后即可管理你的文件。"
            },
        );
        ui.add_space(6.0);
        ui.label("保险库目录");
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2((ui.available_width() - 68.0).max(120.0), 28.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.form.path.ui(ui),
            );
            if ui
                .button("浏览…")
                .on_hover_text("新建时会在所选目录下追加“新保险库”")
                .clicked()
            {
                self.pick(Target::Vault, ui.ctx());
            }
        });
        if self.form.create {
            theme::caption(ui, "填写尚不存在的目录；浏览时自动追加“新保险库”。");
        }
        ui.label(if !self.form.create && self.form.recovery {
            "恢复密钥"
        } else {
            "密码"
        });
        ui.add(
            egui::TextEdit::singleline(&mut *self.form.phrase)
                .char_limit(vault_core::crypto::PASSWORD_MAX)
                .desired_width(f32::INFINITY)
                .margin(egui::Margin::symmetric(8, 5))
                .hint_text(if self.form.create {
                    "设置保险库密码"
                } else {
                    "输入以解锁"
                })
                .password(!self.form.show_password),
        );
        if self.form.create {
            ui.label("确认密码");
            ui.add(
                egui::TextEdit::singleline(&mut *self.form.confirmation)
                    .char_limit(vault_core::crypto::PASSWORD_MAX)
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::symmetric(8, 5))
                    .password(!self.form.show_password),
            );
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.form.show_password, "显示输入");
            if !self.form.create {
                ui.checkbox(&mut self.form.read_only, "只读打开");
            }
        });
        ui.checkbox(
            &mut self.form.recovery,
            if self.form.create {
                "同时生成恢复密钥"
            } else {
                "使用恢复密钥解锁"
            },
        );
        ui.add_space(8.0);
        if ui
            .add_enabled(
                !self.form.path.is_empty(),
                theme::primary(if self.form.create {
                    "创建保险库"
                } else {
                    "解锁保险库"
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
                    self.notice.clear();
                }
                Err(error) => {
                    self.notice = format!("输入或启动失败：{error}（创建时两次密码须一致）。")
                }
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
                self.notice.clear();
                self.last_activity = Some(Instant::now());
            }
            Err(error) => {
                self.lock(ctx);
                self.notice = format!("任务未启动，会话已锁定：{error}");
            }
        }
    }

    fn pick(&mut self, target: Target, ctx: &egui::Context) {
        match Picker::start(target, ctx.clone()) {
            Ok(picker) => self.picker = Some(picker),
            Err(error) => self.notice = format!("无法启动选择器：{error}；可手填路径。"),
        }
    }

    fn browse(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let session = self.session.as_mut().expect("unlocked session");
        if let Some(recovery) = &mut session.recovery {
            let mut saved = false;
            theme::card().show(ui, |ui| {
                ui.heading("保存你的恢复密钥");
                ui.label("恢复密钥仅在此显示一次，请独立保存；它不替代数据备份。");
                let mut text = recovery.as_str();
                ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(8, 5)),
                );
                saved = ui.add(theme::primary("已独立保存，继续")).clicked();
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
                        ui.heading("文件详情");
                        ui.add_space(8.0);
                        self.editor
                            .ui(ui, &session.vault, &self.browser, &mut picker)
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
                    ui.heading("保险库");
                    ui.add_space(8.0);
                    for (trash, label) in [(false, "全部文件"), (true, "回收站")] {
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
                            "可读写会话"
                        } else {
                            "只读会话"
                        },
                    );
                    theme::caption(ui, "本地加密存储");
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BACKGROUND).inner_margin(16))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(if self.browser.trash {
                        "回收站"
                    } else {
                        "我的文件"
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.selectable_label(self.browser.trash, "回收站").clicked() {
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
                        "检测到不完整或损坏引用，请检查后恢复。",
                    );
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.browser.trash, egui::Button::new("根目录"))
                        .clicked()
                    {
                        self.browser.parent = None;
                        self.browser.query = Zeroizing::default();
                        self.browser.selected = None;
                    }
                    if ui
                        .add_enabled(
                            !self.browser.trash && self.browser.parent.is_some(),
                            egui::Button::new("上一级"),
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
                                .map_or("全部文件", |entry| entry.name.as_str()),
                        )
                        .truncate(),
                    );
                });
                ui.add(
                    egui::TextEdit::singleline(&mut *self.browser.query)
                        .hint_text("搜索保险库中的全部名称…")
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
                theme::caption(ui, &format!("{} 个项目 · 双击目录进入", entries.len()));
                ui.separator();
                if entries.is_empty() {
                    ui.add_space(32.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(if !self.browser.query.is_empty() {
                                "没有找到匹配的文件"
                            } else if self.browser.trash {
                                "回收站是空的"
                            } else {
                                "这里还没有文件"
                            })
                            .text_style(egui::TextStyle::Heading),
                        );
                        theme::caption(
                            ui,
                            if self.browser.query.is_empty() && !self.browser.trash {
                                "在右侧展开“导入与新建”以添加文件。"
                            } else {
                                "可以切换目录，或调整搜索内容。"
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
                                    EntryKind::Directory => ("目录", "—".into()),
                                    EntryKind::File { length, .. } => ("文件", theme::size(length)),
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
                                    if ui.small_button("打开目录").clicked() {
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
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("退出").on_hover_text("Ctrl+Q").clicked() {
                            self.tick(&ctx, Instant::now(), true, true);
                        }
                        if (self.session.is_some() || self.task.is_some() || self.picker.is_some())
                            && ui.button("锁定 / 取消").clicked()
                        {
                            self.lock(&ctx);
                        }
                        theme::caption(
                            ui,
                            if self.session.is_some() {
                                "已解锁"
                            } else {
                                "已锁定"
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
                        egui::RichText::new("实验版本")
                            .size(12.0)
                            .color(theme::MUTED),
                    )
                    .on_hover_text("格式未稳定，未经独立安全审计，仅使用可丢弃测试数据。");
                    if self.picker.is_some() {
                        theme::caption(ui, "等待系统选择器；锁定可丢弃选择结果。");
                    } else if !self.notice.is_empty() {
                        ui.label(&self.notice);
                    } else {
                        theme::caption(ui, "空闲 5 分钟自动锁定");
                    }
                });
            });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BACKGROUND)).show(ui, |ui| {
            if let Some(task) = &self.task {
                egui::Frame::new().inner_margin(24).show(ui, |ui| {
                    ui.heading("正在处理");
                    if let Some(progress) = task.progress().filter(|progress| progress.total > 0) {
                        ui.add(egui::ProgressBar::new(progress.processed as f32 / progress.total as f32)
                            .animate(false).text(format!("{} / {}", theme::size(progress.processed), theme::size(progress.total))));
                    }
                    ui.label("取消或关闭会等待任务安全结束；已提交操作不会回滚。密码计算不能立即中断。");
                });
            } else if self.session.is_some() {
                let action = ui.add_enabled_ui(self.picker.is_none(), |ui| self.browse(ui)).inner;
                if let Some(action) = action { self.start_action(action, &ctx); }
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
