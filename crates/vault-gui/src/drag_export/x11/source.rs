//! Xdnd protocol state and selection serving, independent of GUI scheduling.
use super::Result;
use std::{
    collections::VecDeque,
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};
use x11rb::{
    COPY_DEPTH_FROM_PARENT, NONE,
    connection::Connection,
    protocol::{Event, xproto::*},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

x11rb::atom_manager! {
    pub(super) Atoms: AtomsCookie {
        XdndAware, XdndProxy, XdndSelection, XdndEnter, XdndPosition,
        XdndStatus, XdndLeave, XdndDrop, XdndFinished, XdndActionCopy,
        XdndActionList, TARGETS, Uri: b"text/uri-list",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Target {
    window: Window,
    proxy: Window,
    version: u32,
}

pub(super) struct Source {
    conn: RustConnection,
    atoms: Atoms,
    source: Window,
    app: Window,
    root: Window,
    time: Timestamp,
    uri: Vec<u8>,
    target: Option<Target>,
    accepted: bool,
    pending: bool,
    sent_position: Option<u32>,
    location: u32,
    released: Option<Instant>,
    dropped: bool,
    awaiting_release: Option<Instant>,
    phase: Arc<AtomicU8>,
    queued: VecDeque<Event>,
}

impl Source {
    pub(super) fn new(
        conn: RustConnection,
        app: Window,
        path: &Path,
        phase: Arc<AtomicU8>,
    ) -> Result<Self> {
        let root = conn.get_geometry(app)?.reply()?.root;
        let atoms = Atoms::new(&conn)?.reply()?;
        let source = conn.generate_id()?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            source,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?
        .check()?;
        // Obtain a real server timestamp, without using winit's private event data.
        conn.change_property32(
            PropMode::REPLACE,
            source,
            atoms.XdndActionList,
            AtomEnum::ATOM,
            &[atoms.XdndActionCopy],
        )?
        .check()?;
        let time = loop {
            if let Event::PropertyNotify(event) = conn.wait_for_event()? {
                break event.time;
            }
        };
        conn.set_selection_owner(source, atoms.XdndSelection, time)?
            .check()?;
        if conn
            .get_selection_owner(atoms.XdndSelection)?
            .reply()?
            .owner
            != source
        {
            return Err("could not own drag selection".into());
        }
        if !path.is_absolute() {
            return Err("file path must be absolute".into());
        }
        const ESCAPES: percent_encoding::AsciiSet = percent_encoding::NON_ALPHANUMERIC
            .remove(b'/')
            .remove(b'-')
            .remove(b'.')
            .remove(b'_')
            .remove(b'~');
        let uri = format!(
            "file://{}\r\n",
            percent_encoding::percent_encode(path.as_os_str().as_bytes(), &ESCAPES)
        )
        .into_bytes();
        Ok(Self {
            conn,
            atoms,
            source,
            app,
            root,
            time,
            uri,
            target: None,
            accepted: false,
            pending: false,
            sent_position: None,
            location: 0,
            released: None,
            dropped: false,
            awaiting_release: None,
            phase,
            queued: VecDeque::new(),
        })
    }

    pub(super) fn poll(&mut self, cancel: bool, release: Option<(i16, i16)>) -> Option<u8> {
        // Once Drop was sent, retain the plaintext even if the target disappears
        // or times out: it may already be copying asynchronously.
        self.advance(cancel, release)
            .unwrap_or(Some(if self.dropped { 4 } else { 3 }))
    }

    fn advance(&mut self, cancel: bool, release: Option<(i16, i16)>) -> Result<Option<u8>> {
        if cancel && !self.dropped {
            return Ok(Some(2));
        }
        for _ in 0..128 {
            let event = if let Some(event) = self.queued.pop_front() {
                Some(event)
            } else {
                self.conn.poll_for_event()?
            };
            let Some(event) = event else {
                break;
            };
            match event {
                Event::SelectionRequest(request) => self.selection(request)?,
                Event::SelectionClear(_) => return Ok(Some(if self.dropped { 4 } else { 2 })),
                Event::ClientMessage(event)
                    if event.format == 32 && event.window == self.source =>
                {
                    let data = event.data.as_data32();
                    if self.target.is_some_and(|target| target.window == data[0]) {
                        if event.type_ == self.atoms.XdndStatus && !self.dropped {
                            self.pending = false;
                            self.accepted =
                                data[1] & 1 != 0 && data[4] == self.atoms.XdndActionCopy;
                        } else if event.type_ == self.atoms.XdndFinished && self.dropped {
                            let success = self.target.unwrap().version < 5
                                || (data[1] & 1 != 0 && data[2] == self.atoms.XdndActionCopy);
                            return Ok(Some(if success { 1 } else { 4 }));
                        }
                    }
                }
                Event::Error(error) => return Err(format!("X11 error: {error:?}").into()),
                _ => {}
            }
        }
        if !self.dropped && self.released.is_none() {
            let pointer = self.conn.query_pointer(self.root)?.reply()?;
            if !pointer.same_screen {
                return Ok(Some(2));
            }
            let (x, y) = if let Some(position) = release {
                self.released = Some(Instant::now());
                position
            } else if !pointer.mask.contains(KeyButMask::BUTTON1) || self.awaiting_release.is_some()
            {
                // Never infer a drop position from a later pointer query. Wait for
                // the release captured by winit, or cancel if it never arrives.
                if self
                    .awaiting_release
                    .get_or_insert_with(Instant::now)
                    .elapsed()
                    > Duration::from_secs(2)
                {
                    return Ok(Some(2));
                }
                self.conn.flush()?;
                return Ok(None);
            } else {
                (pointer.root_x, pointer.root_y)
            };
            let target = self.target_at(x, y)?;
            self.location = (u32::from(x as u16) << 16) | u32::from(y as u16);
            if target != self.target {
                self.leave();
                self.target = target;
                self.accepted = false;
                self.pending = false;
                self.sent_position = None;
                if let Some(target) = target {
                    self.send(
                        self.atoms.XdndEnter,
                        [self.source, target.version << 24, self.atoms.Uri, 0, 0],
                    )?;
                }
            }
        }
        if !self.dropped
            && self.target.is_some()
            && !self.pending
            && self.sent_position != Some(self.location)
        {
            self.time = self.server_time()?;
            self.send(
                self.atoms.XdndPosition,
                [
                    self.source,
                    0,
                    self.location,
                    self.time,
                    self.atoms.XdndActionCopy,
                ],
            )?;
            self.sent_position = Some(self.location);
            self.pending = true;
        }
        if let Some(released) = self.released {
            if self.dropped {
                if released.elapsed() > Duration::from_secs(30) {
                    return Ok(Some(4));
                }
            } else if self.target.is_none()
                || (!self.pending && !self.accepted)
                || released.elapsed() > Duration::from_secs(2)
            {
                return Ok(Some(2));
            } else if !self.pending && self.accepted {
                self.time = self.server_time()?;
                if self
                    .phase
                    .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    return Ok(Some(2));
                }
                self.dropped = true;
                self.send(self.atoms.XdndDrop, [self.source, 0, self.time, 0, 0])?;
            }
        }
        self.conn.flush()?;
        Ok(None)
    }

    fn server_time(&mut self) -> Result<Timestamp> {
        self.conn
            .change_property32(
                PropMode::REPLACE,
                self.source,
                self.atoms.XdndActionList,
                AtomEnum::ATOM,
                &[self.atoms.XdndActionCopy],
            )?
            .check()?;
        loop {
            let event = self.conn.wait_for_event()?;
            if let Event::PropertyNotify(event) = &event
                && event.window == self.source
                && event.atom == self.atoms.XdndActionList
            {
                return Ok(event.time);
            }
            if self.queued.len() >= 128 {
                return Err("too many pending X11 events".into());
            }
            self.queued.push_back(event);
        }
    }

    fn property(&self, window: Window, name: Atom, kind: AtomEnum) -> Result<Option<u32>> {
        let reply = self
            .conn
            .get_property(false, window, name, kind, 0, 1)?
            .reply()?;
        Ok(reply.value32().and_then(|mut values| values.next()))
    }

    fn target_at(&self, x: i16, y: i16) -> Result<Option<Target>> {
        let mut window = self
            .conn
            .translate_coordinates(self.root, self.root, x, y)?
            .reply()?
            .child;
        if window == NONE {
            window = self.root;
        }
        // Walk the actual pointer hierarchy, including a window manager's frame.
        for _ in 0..32 {
            if window == NONE || (window == self.app && window != self.root) {
                return Ok(None);
            }
            let proxy = self
                .property(window, self.atoms.XdndProxy, AtomEnum::WINDOW)?
                .filter(|&proxy| {
                    self.property(proxy, self.atoms.XdndProxy, AtomEnum::WINDOW)
                        .ok()
                        .flatten()
                        == Some(proxy)
                })
                .unwrap_or(window);
            if let Some(version) = self.property(proxy, self.atoms.XdndAware, AtomEnum::ATOM)?
                && version >= 3
            {
                return Ok(Some(Target {
                    window,
                    proxy,
                    version: version.min(5),
                }));
            }
            if window == self.root {
                return Ok(None);
            }
            window = self
                .conn
                .translate_coordinates(self.root, window, x, y)?
                .reply()?
                .child;
        }
        Ok(None)
    }

    fn send(&self, kind: Atom, data: [u32; 5]) -> Result<()> {
        if let Some(target) = self.target {
            self.conn.send_event(
                false,
                target.proxy,
                EventMask::NO_EVENT,
                ClientMessageEvent::new(32, target.window, kind, data),
            )?;
        }
        Ok(())
    }

    fn leave(&self) {
        if !self.dropped {
            let _ = self.send(self.atoms.XdndLeave, [self.source, 0, 0, 0, 0]);
        }
    }

    fn selection(&self, request: SelectionRequestEvent) -> Result<()> {
        let property = if request.property == NONE {
            request.target
        } else {
            request.property
        };
        let supported =
            request.selection == self.atoms.XdndSelection && request.owner == self.source;
        let property = if supported && request.target == self.atoms.Uri {
            self.conn.change_property8(
                PropMode::REPLACE,
                request.requestor,
                property,
                self.atoms.Uri,
                &self.uri,
            )?;
            property
        } else if supported && request.target == self.atoms.TARGETS {
            self.conn.change_property32(
                PropMode::REPLACE,
                request.requestor,
                property,
                AtomEnum::ATOM,
                &[self.atoms.Uri, self.atoms.TARGETS],
            )?;
            property
        } else {
            NONE
        };
        self.conn.send_event(
            false,
            request.requestor,
            EventMask::NO_EVENT,
            SelectionNotifyEvent {
                response_type: SELECTION_NOTIFY_EVENT,
                sequence: 0,
                time: request.time,
                requestor: request.requestor,
                selection: request.selection,
                target: request.target,
                property,
            },
        )?;
        Ok(())
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.leave();
        let _ = self.conn.destroy_window(self.source);
        let _ = self.conn.flush();
        // Closing this dedicated connection also relinquishes XdndSelection.
    }
}

#[cfg(test)]
#[path = "../x11_source_tests.rs"]
mod tests;
