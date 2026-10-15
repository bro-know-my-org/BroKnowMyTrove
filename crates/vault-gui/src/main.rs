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
use actions::Action;
use picker::{PathInput, Picker, Target};
use session::{Request, Session, Task};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([800.0, 600.0]),
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
            cc.egui_ctx.set_fonts(fonts);
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
        ctx.memory_mut(|memory| *memory = Default::default());
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
        ui.label("保险库目录（新建时填写不存在的目录）：");
        self.form.path.ui(ui);
        if ui.button("选择目录（创建时追加“新保险库”）").clicked() {
            self.pick(Target::Vault, ui.ctx());
        }
        ui.checkbox(&mut self.form.create, "创建新保险库");
        if !self.form.create {
            ui.checkbox(
                &mut self.form.read_only,
                "只读打开（损坏引用时可检查/导出）",
            );
        }
        ui.checkbox(
            &mut self.form.recovery,
            if self.form.create {
                "生成恢复密钥"
            } else {
                "使用恢复密钥解锁"
            },
        );
        ui.label(if !self.form.create && self.form.recovery {
            "恢复密钥："
        } else {
            "密码："
        });
        ui.add(
            egui::TextEdit::singleline(&mut *self.form.phrase)
                .char_limit(vault_core::crypto::PASSWORD_MAX)
                .password(!self.form.show_password),
        );
        if self.form.create {
            ui.label("再次输入密码：");
            ui.add(
                egui::TextEdit::singleline(&mut *self.form.confirmation)
                    .char_limit(vault_core::crypto::PASSWORD_MAX)
                    .password(!self.form.show_password),
            );
        }
        ui.checkbox(&mut self.form.show_password, "显示输入以核对");
        if ui
            .add_enabled(
                !self.form.path.is_empty(),
                egui::Button::new(if self.form.create { "创建" } else { "解锁" }),
            )
            .clicked()
        {
            let result = self
                .form
                .request()
                .and_then(|request| Task::start(self.form.path.path(), request, ui.ctx().clone()));
            ui.ctx().memory_mut(|memory| *memory = Default::default());
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
            ui.label("恢复密钥仅在此显示一次，请独立保存；它不替代数据备份。");
            let mut text = recovery.as_str();
            ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY));
            if ui.button("已独立保存，继续").clicked() {
                session.recovery = None;
                ui.ctx().memory_mut(|memory| *memory = Default::default());
            }
            return None;
        }
        ui.label(if session.vault.is_writable() {
            "已解锁，可修改；空闲 5 分钟会取消任务并锁定。"
        } else {
            "只读会话：可浏览、检查与导出。"
        });
        if !session.vault.issues().is_empty() {
            ui.colored_label(
                egui::Color32::YELLOW,
                "检测到不完整或损坏引用，请用 CLI inspect / check 检查后恢复。",
            );
        }
        ui.horizontal(|ui| {
            if ui.button("根目录").clicked() {
                self.browser.parent = None;
                self.browser.query = Zeroizing::default();
            }
            if ui.button("上一级").clicked() {
                self.browser.parent = self
                    .browser
                    .parent
                    .and_then(|id| session.vault.entry(id))
                    .and_then(|entry| entry.parent);
                self.browser.query = Zeroizing::default();
            }
            ui.checkbox(&mut self.browser.trash, "回收站（平铺）");
        });
        let action = self.editor.ui(ui, &session.vault, &self.browser);
        ui.label(
            self.browser
                .parent
                .and_then(|id| session.vault.entry(id))
                .map_or("根目录", |entry| entry.name.as_str()),
        );
        ui.horizontal(|ui| {
            ui.label("搜索全部名称：");
            ui.text_edit_singleline(&mut *self.browser.query);
        });
        let entries: Vec<_> = session
            .vault
            .search(&self.browser.query, self.browser.trash)
            .filter(|entry| {
                self.browser.trash
                    || !self.browser.query.is_empty()
                    || entry.parent == self.browser.parent
            })
            .collect();
        ui.label(format!("{} 项", entries.len()));
        egui::ScrollArea::vertical().show_rows(ui, 24.0, entries.len(), |ui, rows| {
            for index in rows {
                let entry = entries[index];
                ui.horizontal(|ui| {
                    match entry.kind {
                        EntryKind::Directory => {
                            if ui
                                .add_enabled(!self.browser.trash, egui::Button::new("打开目录"))
                                .clicked()
                            {
                                self.browser.parent = Some(entry.id);
                                self.browser.query = Zeroizing::default();
                            }
                        }
                        EntryKind::File { length, .. } => {
                            ui.label(format!("{length} 字节"));
                        }
                    }
                    if ui
                        .selectable_label(self.browser.selected == Some(entry.id), &entry.name)
                        .clicked()
                    {
                        self.browser.selected = Some(entry.id);
                        self.editor.name = Zeroizing::new(entry.name.clone());
                    }
                });
            }
        });
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
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading(vault_core::PRODUCT_NAME);
            ui.label("实验版本，仅使用可丢弃测试数据；格式未稳定，未经独立安全审计。");
            ui.horizontal(|ui| {
                if (self.session.is_some() || self.task.is_some() || self.picker.is_some())
                    && ui.button("锁定 / 取消任务").clicked()
                {
                    self.lock(&ctx);
                }
                if ui.button("关闭 (Ctrl+Q)").clicked() {
                    self.tick(&ctx, Instant::now(), true, true);
                }
            });
            ui.separator();
            ui.label(&self.notice);
            if let Some(task) = &self.task {
                ui.spinner();
                if let Some(progress) = task.progress().filter(|progress| progress.total > 0) {
                    ui.add(
                        egui::ProgressBar::new(progress.processed as f32 / progress.total as f32)
                            .text(format!("{} / {} 字节", progress.processed, progress.total)),
                    );
                }
                ui.label(
                    "正在处理；取消/关闭等待安全结束，已提交操作不会回滚。密码计算不能立即中断。",
                );
            } else if self.session.is_some() {
                ui.add_enabled_ui(self.picker.is_none(), |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("选择导入/替换源文件").clicked() {
                            self.pick(Target::Source, &ctx);
                        }
                        if ui.button("选择导出目标").clicked() {
                            self.pick(Target::Destination, &ctx);
                        }
                    });
                    if let Some(action) = self.browse(ui) {
                        self.start_action(action, &ctx);
                    }
                });
            } else if !self.close_when_done {
                ui.add_enabled_ui(self.picker.is_none(), |ui| self.login(ui));
            }
            if self.picker.is_some() {
                ui.label("等待系统选择器；锁定会丢弃选择结果，可在系统对话框中取消。");
            }
        });
    }
}

#[cfg(test)]
mod tests;
