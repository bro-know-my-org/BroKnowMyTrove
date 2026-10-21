use crate::session::Session;
use eframe::egui;
use std::{
    sync::{Arc, mpsc},
    thread,
};
use vault_core::{Entry, Error, Id, Result, SearchIndex, operation::Operation};
use zeroize::Zeroizing;

#[derive(Default)]
pub(super) struct Browser {
    pub query: Zeroizing<String>,
    pub parent: Option<Id>,
    pub trash: bool,
    pub selected: Option<Id>,
    pub cached: Option<Listing>,
    pub worker: Option<SearchTask>,
}

pub(super) struct Listing {
    state: (Option<Id>, Option<Id>, bool),
    query: Zeroizing<String>,
    rows: Vec<usize>,
}

pub(super) struct SearchTask {
    operation: Operation,
    receiver: mpsc::Receiver<Result<Listing>>,
    worker: Option<thread::JoinHandle<()>>,
    state: (Option<Id>, Option<Id>, bool),
    query: Zeroizing<String>,
}

impl Browser {
    pub fn pending(&self) -> bool {
        self.worker.is_some()
    }

    pub fn entries<'a>(
        &mut self,
        session: &'a Session,
        ctx: &egui::Context,
    ) -> Result<Vec<&'a Entry>> {
        let state = (session.vault.current_commit(), self.parent, self.trash);
        if let Some(worker) = &self.worker {
            if worker.state != state || worker.query.as_str() != self.query.as_str() {
                worker.operation.cancel();
            }
            match worker.receiver.try_recv() {
                Ok(result) => {
                    self.worker = None;
                    if let Ok(listing) = result
                        && listing.state == state
                        && listing.query.as_str() == self.query.as_str()
                    {
                        self.cached = Some(listing);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.worker = None;
                    return Err(Error::Crypto);
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.cached.as_ref().is_some_and(|cache| {
            cache.state == state && cache.query.as_str() == self.query.as_str()
        }) {
            return Ok(self
                .cached
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .filter_map(|&row| session.vault.entries().nth(row))
                .collect());
        }
        self.cached = None;
        if self.worker.is_none() && session.search.commit() == state.0 {
            self.worker = Some(SearchTask::start(
                session.search.clone(),
                self.query.clone(),
                state,
                ctx.clone(),
            )?);
        }
        Ok(Vec::new())
    }
}

impl SearchTask {
    fn start(
        index: Arc<SearchIndex>,
        query: Zeroizing<String>,
        state: (Option<Id>, Option<Id>, bool),
        ctx: egui::Context,
    ) -> std::io::Result<Self> {
        let operation = Operation::default();
        let worker_operation = operation.clone();
        let worker_query = query.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new().spawn(move || {
            let result = index
                .rows(&worker_query, state.1, state.2, &worker_operation)
                .map(|rows| Listing {
                    state,
                    query: worker_query,
                    rows,
                });
            let _ = sender.send(result);
            ctx.request_repaint();
        })?;
        Ok(Self {
            operation,
            receiver,
            worker: Some(worker),
            state,
            query,
        })
    }
}

impl Drop for SearchTask {
    fn drop(&mut self) {
        self.operation.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
