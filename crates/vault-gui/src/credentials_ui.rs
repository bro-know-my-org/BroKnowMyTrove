use crate::{actions::Action, theme};
use eframe::egui;
use vault_core::{Error, Result, crypto::Password};
use vault_i18n::{ErrorContext, Locale, Message, TextKey};
use zeroize::Zeroizing;

#[derive(Default)]
pub struct Credentials {
    pub current: Zeroizing<String>,
    pub new: Zeroizing<String>,
    pub confirmation: Zeroizing<String>,
    pub invalid: bool,
}

impl Credentials {
    pub fn prepare(&mut self, kind: TextKey) -> Result<Action> {
        let mut current = std::mem::take(&mut self.current);
        let mut new = std::mem::take(&mut self.new);
        let mut confirmation = std::mem::take(&mut self.confirmation);
        self.invalid = false;
        match kind {
            TextKey::ChangePassword => {
                let password = Password::new(std::mem::take(&mut *new))?;
                if !password.matches(&Password::new(std::mem::take(&mut *confirmation))?) {
                    return Err(Error::InvalidInput);
                }
                Ok(Action::ChangePassword(password))
            }
            TextKey::RotateRecovery => Ok(Action::RotateRecovery(Password::new(std::mem::take(
                &mut *current,
            ))?)),
            TextKey::RemoveRecovery => Ok(Action::RemoveRecovery(Password::new(std::mem::take(
                &mut *current,
            ))?)),
            _ => Err(Error::InvalidInput),
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        locale: Locale,
        writable: bool,
    ) -> Option<Result<Action>> {
        let mut result = None;
        egui::CollapsingHeader::new(locale.text(TextKey::Credentials))
            .id_salt("credentials")
            .show(ui, |ui| {
                ui.label(locale.text(TextKey::KeyUpdateWarning));
                ui.add_enabled_ui(writable, |ui| {
                    ui.label(locale.text(TextKey::NewPasswordHint));
                    for (label, text) in [
                        (TextKey::SetPassword, &mut self.new),
                        (TextKey::ConfirmPassword, &mut self.confirmation),
                    ] {
                        ui.label(locale.text(label));
                        ui.add(
                            egui::TextEdit::singleline(&mut **text)
                                .password(true)
                                .desired_width(f32::INFINITY),
                        );
                    }
                    let mut selected = None;
                    if ui.button(locale.text(TextKey::ChangePassword)).clicked() {
                        selected = Some(TextKey::ChangePassword);
                    }
                    ui.separator();
                    ui.label(locale.text(TextKey::KnownPasswordRequired));
                    ui.add(
                        egui::TextEdit::singleline(&mut *self.current)
                            .password(true)
                            .desired_width(f32::INFINITY),
                    );
                    for kind in [TextKey::RotateRecovery, TextKey::RemoveRecovery] {
                        if ui.button(locale.text(kind)).clicked() {
                            selected = Some(kind);
                        }
                    }
                    if let Some(kind) = selected {
                        result = Some(self.prepare(kind));
                        theme::clear_widget_memory(ui.ctx());
                    }
                });
                if self.invalid {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        Message::Error(ErrorContext::Input, Error::InvalidInput).render(locale),
                    );
                }
            });
        result
    }
}
