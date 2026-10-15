use eframe::egui;
use std::time::{Duration, Instant};
use vault_core::{
    CreateOptions, EntryKind, Error, Id, Result,
    crypto::{Password, RecoveryKey},
};
use zeroize::Zeroizing;

mod session;
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
    path: String,
    phrase: Zeroizing<String>,
    confirmation: Zeroizing<String>,
    create: bool,
    recovery: bool,
    show_password: bool,
}

impl Form {
    fn request(&mut self) -> Result<Request> {
        let mut phrase = std::mem::take(&mut self.phrase);
        let mut confirmation = std::mem::take(&mut self.confirmation);
        self.show_password = false;
        if !self.create && self.recovery {
            return Ok(Request::Recover(RecoveryKey::decode(&phrase)?));
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
            Ok(Request::Open(password))
        }
    }
}

#[derive(Default)]
struct Browser {
    query: Zeroizing<String>,
    parent: Option<Id>,
    trash: bool,
}

#[derive(Default)]
struct VaultApp {
    form: Form,
    browser: Browser,
    session: Option<Session>,
    task: Option<Task>,
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
        self.browser = Browser::default();
        self.form.phrase = Zeroizing::default();
        self.form.confirmation = Zeroizing::default();
        self.form.show_password = false;
        // Also drop widget editing/undo state. No eframe persistence is enabled.
        ctx.memory_mut(|memory| *memory = Default::default());
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.task.as_ref().and_then(Task::poll) {
            self.task = None; // Join before accepting another operation.
            if self.discard_result {
                drop(result);
                self.notice = "已停止并锁定；创建可能已完成，请重新打开核查。".into();
            } else {
                match result {
                    Ok(session) => {
                        self.session = Some(session);
                        self.form.create = false;
                        self.form.recovery = false;
                        self.notice.clear();
                        self.last_activity = Some(Instant::now());
                    }
                    Err(error) => {
                        self.notice =
                            format!("{error}；若正在创建，请检查目录，已创建的库仍可用密码打开。")
                    }
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
        if self.session.is_some()
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
        ui.text_edit_singleline(&mut self.form.path);
        ui.checkbox(&mut self.form.create, "创建新保险库");
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
            let result = self.form.request().and_then(|request| {
                Task::start(self.form.path.clone().into(), request, ui.ctx().clone())
            });
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

    fn browse(&mut self, ui: &mut egui::Ui) {
        let session = self.session.as_mut().expect("unlocked session");
        if let Some(recovery) = &mut session.recovery {
            ui.label("恢复密钥仅在此显示一次，请独立保存；它不替代数据备份。");
            let mut text = recovery.as_str();
            ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY));
            if ui.button("已独立保存，继续").clicked() {
                session.recovery = None;
                ui.ctx().memory_mut(|memory| *memory = Default::default());
            }
            return;
        }
        ui.label("当前提供只读浏览；文件修改请使用 CLI。空闲 5 分钟自动锁定。");
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
                    ui.label(&entry.name);
                });
            }
        });
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
                if (self.session.is_some() || self.task.is_some()) && ui.button("锁定 / 取消任务").clicked() { self.lock(&ctx); }
                if ui.button("关闭 (Ctrl+Q)").clicked() { self.tick(&ctx, Instant::now(), true, true); }
            });
            ui.separator();
            ui.label(&self.notice);
            if self.task.is_some() {
                ui.spinner();
                ui.label("正在处理；密码计算不能立即中断。取消或关闭会等待安全结束并释放锁，创建可能已提交。");
            } else if self.session.is_some() {
                self.browse(ui);
            } else if !self.close_when_done {
                self.login(ui);
            }
        });
    }
}

#[cfg(test)]
mod tests;
