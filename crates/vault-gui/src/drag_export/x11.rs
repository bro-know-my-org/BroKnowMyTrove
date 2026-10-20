//! Xdnd copy source on a separate X11 connection. The existing winit implicit
//! pointer grab stays intact; querying the pointer also works outside our window.
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    os::fd::{AsFd, OwnedFd},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use x11rb::{
    protocol::xproto::Window,
    rust_connection::{DefaultStream, RustConnection},
};

mod source;
#[cfg(test)]
use source::Atoms;
use source::Source;
#[cfg(test)]
static X11_TEST_LOCK: Mutex<()> = Mutex::new(());

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn window(frame: &eframe::Frame) -> Option<Window> {
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok(),
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        _ => None, // A native Wayland window cannot initiate an X11 drag.
    }
}

pub fn local_display(name: Option<&str>) -> bool {
    use x11rb::reexports::x11rb_protocol::parse_display::{ConnectAddress, parse_display};
    parse_display(name).is_ok_and(|display| {
        display
            .connect_instruction()
            .any(|address| matches!(address, ConnectAddress::Socket(_)))
    })
}

enum Input {
    Release(i16, i16),
    Cancel,
}

struct Control {
    canceled: AtomicBool,
    completed: AtomicBool,
    phase: Arc<AtomicU8>, // 0: dragging, 1: committed, 2: canceled before commit.
    socket: Mutex<Option<OwnedFd>>,
    progress: Mutex<Instant>,
    cancel_at: Mutex<Option<Instant>>,
}

impl Control {
    fn new() -> Self {
        Self {
            canceled: AtomicBool::new(false),
            completed: AtomicBool::new(false),
            phase: Arc::default(),
            socket: Mutex::new(None),
            progress: Mutex::new(Instant::now()),
            cancel_at: Mutex::new(None),
        }
    }

    fn cancel(&self) {
        let _ = self
            .phase
            .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst);
        self.canceled.store(true, Ordering::SeqCst);
        self.cancel_at
            .lock()
            .unwrap()
            .get_or_insert_with(Instant::now);
    }

    fn shutdown(&self) {
        if let Some(socket) = self.socket.lock().unwrap().as_ref() {
            let _ = rustix::net::shutdown(socket, rustix::net::Shutdown::Both);
        }
    }

    fn handshake(
        &self,
        stream: DefaultStream,
        screen: usize,
        auth: (Vec<u8>, Vec<u8>),
    ) -> Result<RustConnection> {
        {
            let mut socket = self.socket.lock().unwrap();
            if self.canceled.load(Ordering::SeqCst) {
                return Err("drag canceled".into());
            }
            *socket = Some(stream.as_fd().try_clone_to_owned()?);
        }
        Ok(RustConnection::connect_to_stream_with_auth_info(
            stream, screen, auth.0, auth.1,
        )?)
    }

    fn connect(&self) -> Result<RustConnection> {
        use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
        use x11rb::reexports::x11rb_protocol::{
            parse_display::{ConnectAddress, parse_display},
            xauth::get_auth,
        };
        let display = parse_display(None)?;
        // This backend supports local X11/XWayland sockets. Nonblocking connect
        // avoids hanging on a full accept queue; publish the fd BEFORE setup.
        for address in display.connect_instruction() {
            let ConnectAddress::Socket(path) = address else {
                continue;
            };
            for address in [
                SocketAddrUnix::new_abstract_name(path.as_bytes()),
                SocketAddrUnix::new(path),
            ] {
                let socket = rustix::net::socket_with(
                    AddressFamily::UNIX,
                    SocketType::STREAM,
                    SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
                    None,
                )?;
                if rustix::net::connect(&socket, &address?).is_err() {
                    continue;
                }
                let (stream, (family, address)) = DefaultStream::from_unix_stream(socket.into())?;
                let auth = get_auth(family, &address, display.display)
                    .unwrap_or(None)
                    .unwrap_or_default();
                return self.handshake(stream, display.screen.into(), auth);
            }
        }
        Err("no local X11 socket available".into())
    }

    fn run(&self, app: Window, path: &Path, receiver: mpsc::Receiver<Input>) -> Result<u8> {
        let mut source = Source::new(self.connect()?, app, path, self.phase.clone())?;
        loop {
            if self.canceled.load(Ordering::SeqCst) {
                return Ok(if self.phase.load(Ordering::SeqCst) == 1 {
                    4
                } else {
                    2
                });
            }
            *self.progress.lock().unwrap() = Instant::now();
            let mut cancel = false;
            let mut release = None;
            for input in receiver.try_iter() {
                match input {
                    Input::Cancel => cancel = true,
                    Input::Release(x, y) => {
                        release.get_or_insert((x, y));
                    }
                }
            }
            if let Some(result) = source.poll(cancel, release) {
                return Ok(result);
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    }
}

pub struct Drag {
    control: Arc<Control>,
    sender: mpsc::Sender<Input>,
    outcome: Arc<AtomicU8>,
}

impl Drag {
    pub fn start(
        frame: &eframe::Frame,
        path: &Path,
        outcome: Arc<AtomicU8>,
        ctx: eframe::egui::Context,
    ) -> Result<Self> {
        let app = window(frame).ok_or("not an X11 window")?;
        Self::start_window(app, path, outcome, ctx)
    }

    fn start_window(
        app: Window,
        path: &Path,
        outcome: Arc<AtomicU8>,
        ctx: eframe::egui::Context,
    ) -> Result<Self> {
        let path = path.to_owned();
        let control = Arc::new(Control::new());
        let worker_control = control.clone();
        let worker_outcome = outcome.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("vault-xdnd".into())
            .spawn(move || {
                let result = worker_control.run(app, &path, receiver).unwrap_or(
                    if worker_control.phase.load(Ordering::SeqCst) == 1 {
                        4
                    } else {
                        3
                    },
                );
                if !worker_control.completed.swap(true, Ordering::SeqCst) {
                    worker_outcome.store(result, Ordering::SeqCst);
                }
                ctx.request_repaint();
            })?;
        Ok(Self {
            control,
            sender,
            outcome,
        })
    }

    pub fn poll(&self) {
        let canceled = self
            .control
            .cancel_at
            .lock()
            .unwrap()
            .is_some_and(|time| time.elapsed() > Duration::from_millis(200));
        let stalled = self.control.progress.lock().unwrap().elapsed() > Duration::from_secs(3);
        if (canceled || stalled) && !self.control.completed.swap(true, Ordering::SeqCst) {
            self.control.cancel();
            self.control.shutdown();
            self.outcome.store(
                if self.control.phase.load(Ordering::SeqCst) == 1 {
                    4
                } else if canceled {
                    2
                } else {
                    3
                },
                Ordering::SeqCst,
            );
        }
    }

    pub fn input(&self, ctx: &eframe::egui::Context, raw: &eframe::egui::RawInput) {
        use eframe::egui::{Event, Key, PointerButton};
        for event in &raw.events {
            let input = match event {
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: false,
                    ..
                } => {
                    let Some(rect) = raw
                        .viewports
                        .get(&raw.viewport_id)
                        .and_then(|v| v.inner_rect)
                    else {
                        if self
                            .control
                            .phase
                            .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                        {
                            self.control.cancel();
                        }
                        let _ = self.sender.send(Input::Cancel);
                        return;
                    };
                    let position = (rect.min + pos.to_vec2()) * ctx.pixels_per_point();
                    Input::Release(position.x.round() as i16, position.y.round() as i16)
                }
                Event::Key {
                    key: Key::Escape,
                    pressed: true,
                    ..
                } => {
                    if self
                        .control
                        .phase
                        .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        self.control.cancel();
                    }
                    Input::Cancel
                }
                _ => continue,
            };
            let _ = self.sender.send(input);
        }
    }
}

impl Drop for Drag {
    fn drop(&mut self) {
        self.control.cancel();
        if !self.control.completed.load(Ordering::SeqCst) {
            // Let a responsive worker flush XdndLeave. A stalled connection must
            // still be closed promptly, including after the GUI has exited.
            let control = self.control.clone();
            if std::thread::Builder::new()
                .name("vault-xdnd-stop".into())
                .spawn(move || {
                    std::thread::sleep(Duration::from_millis(200));
                    control.shutdown();
                })
                .is_err()
            {
                self.control.shutdown();
            }
        }
    }
}

#[cfg(test)]
#[path = "x11_tests.rs"]
mod tests;
