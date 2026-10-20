use crate::actions::Action;
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
};
use vault_core::{
    BackupVerification, CreateOptions, Credential, Error, OpenMode, Result, Vault,
    crypto::{Password, RecoveryKey},
    operation::{Operation, Progress},
};
use vault_i18n::Message;
use zeroize::Zeroizing;

pub enum Request {
    Create(Password, CreateOptions),
    Open(Password, OpenMode),
    Recover(RecoveryKey, OpenMode),
    RestorePassword(Password, PathBuf, BackupVerification),
    RestoreRecovery(RecoveryKey, PathBuf, BackupVerification),
}

pub struct Session {
    pub vault: Vault,
    pub recovery: Option<Zeroizing<String>>,
    pub drag_export: Option<crate::drag_export::Prepared>,
    pub import_drop: Option<crate::drop_import::Batch>,
}

impl Request {
    fn run(self, path: PathBuf, operation: &Operation) -> Result<Completion> {
        operation.check_cancelled()?;
        let (vault, recovery) = match self {
            Self::Create(password, options) => {
                Vault::create_with_operation(&path, &password, options, operation)?
            }
            Self::Open(password, mode) => (
                Vault::open_with_operation(
                    &path,
                    Credential::Password(&password),
                    mode,
                    operation,
                )?,
                None,
            ),
            Self::Recover(key, mode) => (
                Vault::open_with_operation(&path, Credential::Recovery(&key), mode, operation)?,
                None,
            ),
            Self::RestorePassword(password, destination, verification) => {
                return Ok(Completion {
                    session: None,
                    result: Ok(Message::Backup(Vault::restore_to(
                        &path,
                        Credential::Password(&password),
                        &destination,
                        verification,
                        operation,
                    )?)),
                });
            }
            Self::RestoreRecovery(key, destination, verification) => {
                return Ok(Completion {
                    session: None,
                    result: Ok(Message::Backup(Vault::restore_to(
                        &path,
                        Credential::Recovery(&key),
                        &destination,
                        verification,
                        operation,
                    )?)),
                });
            }
        };
        Ok(Completion {
            session: Some(Session {
                vault,
                recovery: recovery.map(|key| key.encode()).transpose()?,
                drag_export: None,
                import_drop: None,
            }),
            result: Ok(Message::Empty),
        })
    }
}

pub struct Completion {
    pub session: Option<Session>,
    pub result: Result<Message>,
}

/// A single worker owns all disk/KDF work. Dropping a task cancels, then joins it.
pub struct Task {
    pub operation: Operation,
    receiver: mpsc::Receiver<Completion>,
    worker: Option<thread::JoinHandle<()>>,
    progress: Arc<Mutex<Progress>>,
}

impl Task {
    pub fn start(path: PathBuf, request: Request, ctx: egui::Context) -> Result<Self> {
        Self::spawn(ctx, move |operation| match request.run(path, operation) {
            Ok(completion) => completion,
            Err(error) => Completion {
                session: None,
                result: Err(error),
            },
        })
    }

    pub fn action(mut session: Session, action: Action, ctx: egui::Context) -> Result<Self> {
        Self::spawn(ctx, move |operation| {
            let result = action.run(&mut session, operation);
            Completion {
                session: Some(session),
                result,
            }
        })
    }

    fn spawn(
        ctx: egui::Context,
        work: impl FnOnce(&Operation) -> Completion + Send + 'static,
    ) -> Result<Self> {
        let progress = Arc::new(Mutex::new(Progress {
            processed: 0,
            total: 0,
        }));
        let observer = progress.clone();
        let operation = Operation::default().with_progress(move |value| {
            if let Ok(mut progress) = observer.lock() {
                *progress = value;
            }
        });
        let worker_operation = operation.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new().spawn(move || {
            let _ = sender.send(work(&worker_operation));
            ctx.request_repaint();
        })?;
        Ok(Self {
            operation,
            receiver,
            worker: Some(worker),
            progress,
        })
    }

    pub fn progress(&self) -> Option<Progress> {
        self.progress.lock().ok().map(|value| *value)
    }

    pub fn poll(&self) -> Option<Completion> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Completion {
                session: None,
                result: Err(Error::Crypto),
            }),
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        self.operation.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
