use eframe::egui;
use std::{path::PathBuf, sync::mpsc, thread};
use vault_core::{
    CreateOptions, Credential, Error, OpenMode, Result, Vault,
    crypto::{Password, RecoveryKey},
    operation::Operation,
};
use zeroize::Zeroizing;

pub enum Request {
    Create(Password, CreateOptions),
    Open(Password),
    Recover(RecoveryKey),
}

pub struct Session {
    pub vault: Vault,
    pub recovery: Option<Zeroizing<String>>,
}

impl Request {
    fn run(self, path: PathBuf, operation: &Operation) -> Result<Session> {
        operation.check_cancelled()?;
        let (vault, recovery) = match self {
            Self::Create(password, options) => {
                Vault::create_with_operation(&path, &password, options, operation)?
            }
            Self::Open(password) => (
                Vault::open(&path, Credential::Password(&password), OpenMode::ReadOnly)?,
                None,
            ),
            Self::Recover(key) => (
                Vault::open(&path, Credential::Recovery(&key), OpenMode::ReadOnly)?,
                None,
            ),
        };
        Ok(Session {
            vault,
            recovery: recovery.map(|key| key.encode()).transpose()?,
        })
    }
}

/// A single worker owns all disk/KDF work. Dropping a task cancels, then joins it.
pub struct Task {
    pub operation: Operation,
    receiver: mpsc::Receiver<Result<Session>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Task {
    pub fn start(path: PathBuf, request: Request, ctx: egui::Context) -> Result<Self> {
        let operation = Operation::default();
        let worker_operation = operation.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new().spawn(move || {
            let _ = sender.send(request.run(path, &worker_operation));
            ctx.request_repaint();
        })?;
        Ok(Self {
            operation,
            receiver,
            worker: Some(worker),
        })
    }

    pub fn poll(&self) -> Option<Result<Session>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(Error::Crypto)),
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
