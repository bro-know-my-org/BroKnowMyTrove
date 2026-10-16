use std::{
    fs::{File, OpenOptions},
    io::{IsTerminal, Write},
};
use vault_i18n::{Locale, TextKey};

use crossterm::{
    cursor::MoveToColumn,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{self, Clear, ClearType},
};
use unicode_segmentation::UnicodeSegmentation;
use vault_core::{
    Error, Result,
    crypto::{PASSWORD_MAX, Password, RecoveryKey},
};
use zeroize::{Zeroize, Zeroizing};

/// Input always uses a controlling terminal; stdin remains available for file data.
pub fn read_password(confirm: bool) -> Result<Password> {
    read_password_in(confirm, Locale::default())
}

pub fn read_password_in(confirm: bool, locale: Locale) -> Result<Password> {
    let mut terminal = Terminal::open()?;
    let first = Password::new(
        terminal
            .read(locale.text(TextKey::PasswordPrompt))?
            .to_string(),
    )?;
    if confirm {
        let second = Password::new(
            terminal
                .read(locale.text(TextKey::ConfirmPasswordPrompt))?
                .to_string(),
        )?;
        if !first.matches(&second) {
            return Err(Error::InvalidInput);
        }
    }
    Ok(first)
}

pub fn read_recovery() -> Result<RecoveryKey> {
    read_recovery_in(Locale::default())
}

pub fn read_recovery_in(locale: Locale) -> Result<RecoveryKey> {
    let mut terminal = Terminal::open()?;
    RecoveryKey::decode(&terminal.read(locale.text(TextKey::RecoveryPrompt))?)
}

/// Explicit one-time display on the controlling terminal, never stdout or diagnostics.
pub fn show_recovery(key: &RecoveryKey) -> Result<()> {
    show_recovery_in(key, Locale::default())
}

pub fn show_recovery_in(key: &RecoveryKey, locale: Locale) -> Result<()> {
    let mut terminal = Terminal::open()?;
    write!(
        terminal.output,
        "{}{}\r\n",
        locale.text(TextKey::RecoveryDisplay),
        *key.encode()?
    )?;
    terminal.output.flush()?;
    Ok(())
}

struct Terminal {
    output: File,
}

impl Terminal {
    fn open() -> Result<Self> {
        #[cfg(unix)]
        let path = "/dev/tty";
        #[cfg(windows)]
        let path = "CONOUT$";
        let output = OpenOptions::new().read(true).write(true).open(path)?;
        if !output.is_terminal() {
            return Err(Error::InvalidInput);
        }
        terminal::enable_raw_mode()?;
        let mut terminal = Self { output };
        execute!(terminal.output, EnableBracketedPaste)?;
        Ok(terminal)
    }

    fn read(&mut self, prompt: &str) -> Result<Zeroizing<String>> {
        let mut editor = Editor(Zeroizing::new(Vec::with_capacity(PASSWORD_MAX)));
        loop {
            execute!(self.output, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
            write!(self.output, "{prompt}{}", "*".repeat(editor.graphemes()))?;
            self.output.flush()?;
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                    KeyCode::Enter => {
                        write!(self.output, "\r\n")?;
                        return Ok(editor.text());
                    }
                    KeyCode::Esc => return Err(Error::Cancelled),
                    KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Err(Error::Cancelled);
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        editor.0.zeroize()
                    }
                    KeyCode::Backspace => editor.backspace(),
                    KeyCode::Char(ch)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        editor.push(ch)?;
                    }
                    _ => {}
                },
                Event::Paste(paste) => {
                    let paste = Zeroizing::new(paste);
                    for ch in paste.chars() {
                        editor.push(ch)?;
                    }
                }
                _ => {}
            }
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(self.output, DisableBracketedPaste);
        let _ = terminal::disable_raw_mode();
        let _ = writeln!(self.output);
    }
}

struct Editor(Zeroizing<Vec<char>>);

impl Editor {
    fn text(&self) -> Zeroizing<String> {
        let mut text = Zeroizing::new(String::with_capacity(PASSWORD_MAX));
        text.extend(self.0.iter());
        text
    }

    fn graphemes(&self) -> usize {
        self.text().graphemes(true).count()
    }

    fn push(&mut self, ch: char) -> Result<()> {
        if ch.is_control()
            || self.0.iter().map(|ch| ch.len_utf8()).sum::<usize>() + ch.len_utf8() > PASSWORD_MAX
        {
            return Err(Error::InvalidInput);
        }
        self.0.push(ch);
        Ok(())
    }

    fn backspace(&mut self) {
        let text = self.text();
        if let Some((start, _)) = text.grapheme_indices(true).next_back() {
            let keep = text[..start].chars().count();
            self.0[keep..].zeroize();
            self.0.truncate(keep);
        }
    }
}
