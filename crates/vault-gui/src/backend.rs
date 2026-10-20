//! Prefer a working local X11/XWayland connection for file drag-in/out.
//! Leave winit's default selection intact when the authenticated probe fails.
use std::{
    ffi::OsStr,
    io::{self, Read, Write},
    os::{
        fd::{AsFd, OwnedFd},
        unix::net::UnixStream,
    },
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use x11rb::{reexports::x11rb_protocol, rust_connection::DefaultStream};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Backend {
    X11,
    Wayland,
}

fn select(request: Option<&OsStr>, probe: impl FnOnce() -> bool) -> io::Result<Option<Backend>> {
    match request.map(OsStr::to_str) {
        None | Some(Some("auto")) => Ok(probe().then_some(Backend::X11)),
        Some(Some("x11")) => Ok(Some(Backend::X11)),
        Some(Some("wayland")) => Ok(Some(Backend::Wayland)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "BKM_GUI_BACKEND must be auto, x11, or wayland",
        )),
    }
}

pub fn configure(mut options: eframe::NativeOptions) -> eframe::Result<eframe::NativeOptions> {
    use winit::platform::{
        wayland::EventLoopBuilderExtWayland as _, x11::EventLoopBuilderExtX11 as _,
    };
    let backend = select(std::env::var_os("BKM_GUI_BACKEND").as_deref(), || {
        probe_local(None)
    })
    .map_err(|error| eframe::Error::AppCreation(Box::new(error)))?;
    if let Some(backend) = backend {
        options.event_loop_builder = Some(Box::new(move |builder| match backend {
            Backend::X11 => {
                builder.with_x11();
            }
            Backend::Wayland => {
                builder.with_wayland();
            }
        }));
    }
    Ok(options)
}

fn probe_local(display: Option<&str>) -> bool {
    let display = display.map(str::to_owned);
    bounded_probe(Duration::from_millis(250), move |deadline, state| {
        probe_at(display.as_deref(), deadline, &state)
    })
}

#[derive(Default)]
struct ProbeState {
    stopped: bool,
    socket: Option<OwnedFd>,
}

fn bounded_probe(
    timeout: Duration,
    work: impl FnOnce(Instant, Arc<Mutex<ProbeState>>) -> bool + Send + 'static,
) -> bool {
    let deadline = Instant::now() + timeout;
    let state = Arc::new(Mutex::new(ProbeState::default()));
    let worker_state = state.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    // At most one startup probe. A stalled filesystem read may outlive the wait,
    // but owns no GUI/session state and cannot publish a late backend decision.
    if std::thread::Builder::new()
        .name("vault-display-probe".into())
        .spawn(move || {
            let _ = sender.send(work(deadline, worker_state));
        })
        .is_err()
    {
        return false;
    }
    let ready = receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .unwrap_or(false);
    let mut state = state.lock().unwrap();
    state.stopped = true;
    if let Some(socket) = state.socket.take() {
        let _ = rustix::net::shutdown(socket, rustix::net::Shutdown::Both);
    }
    ready
}

fn probe_at(display: Option<&str>, deadline: Instant, state: &Mutex<ProbeState>) -> bool {
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
    use x11rb_protocol::parse_display::{ConnectAddress, parse_display};
    let Ok(display) = parse_display(display) else {
        return false;
    };
    // x11rb constructs TCP fallback ports even for a local display instruction.
    if display.display.checked_add(6000).is_none() {
        return false;
    }
    for address in display.connect_instruction() {
        let ConnectAddress::Socket(path) = address else {
            continue;
        }; // Never probe remote X11.
        for address in [
            SocketAddrUnix::new_abstract_name(path.as_bytes()),
            SocketAddrUnix::new(path),
        ] {
            let Ok(address) = address else {
                continue;
            };
            let Ok(socket) = rustix::net::socket_with(
                AddressFamily::UNIX,
                SocketType::STREAM,
                SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
                None,
            ) else {
                continue;
            };
            // A full accept queue must not block startup.
            if rustix::net::connect(&socket, &address).is_err() {
                continue;
            }
            let Ok((stream, (family, address))) = DefaultStream::from_unix_stream(socket.into())
            else {
                continue;
            };
            {
                let mut state = state.lock().unwrap();
                if state.stopped {
                    return false;
                }
                let Ok(socket) = stream.as_fd().try_clone_to_owned() else {
                    continue;
                };
                state.socket = Some(socket);
            }
            let auth = x11rb_protocol::xauth::get_auth(family, &address, display.display)
                .unwrap_or(None)
                .unwrap_or_default();
            let Ok(socket) = stream.as_fd().try_clone_to_owned() else {
                continue;
            };
            if authenticate(socket.into(), auth, display.screen.into(), deadline).is_ok() {
                return true;
            }
        }
    }
    false
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|time| !time.is_zero())
        .ok_or_else(|| io::ErrorKind::TimedOut.into())
}

fn authenticate(
    mut socket: UnixStream,
    auth: (Vec<u8>, Vec<u8>),
    screen: usize,
    deadline: Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut connect, request) =
        x11rb_protocol::connect::Connect::with_authorization(auth.0, auth.1);
    socket.set_nonblocking(false)?;
    socket.set_write_timeout(Some(remaining(deadline)?))?;
    socket.write_all(&request)?;
    loop {
        socket.set_read_timeout(Some(remaining(deadline)?))?;
        let count = match socket.read(connect.buffer()) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
        }
        if connect.advance(count) || connect.buffer().is_empty() {
            break;
        }
    }
    if connect.into_setup()?.roots.get(screen).is_none() {
        return Err("X11 screen unavailable".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_backends_bypass_probe_and_auto_keeps_default_on_failure() {
        for (request, expected) in [("x11", Backend::X11), ("wayland", Backend::Wayland)] {
            assert_eq!(
                select(Some(OsStr::new(request)), || panic!(
                    "explicit mode must not probe"
                ))
                .unwrap(),
                Some(expected)
            );
        }
        assert_eq!(select(None, || true).unwrap(), Some(Backend::X11));
        assert_eq!(select(None, || false).unwrap(), None);
        assert_eq!(select(Some(OsStr::new("auto")), || false).unwrap(), None);
        assert!(select(Some(OsStr::new("typo")), || true).is_err());
        assert!(!probe_local(Some("localhost:10.0")));
        assert!(!probe_local(Some(":65534")));
    }

    #[test]
    fn stalled_or_rejected_x11_setup_cannot_block_startup() {
        for rejected in [false, true] {
            let (client, mut server) = UnixStream::pair().unwrap();
            let worker = std::thread::spawn(move || {
                server
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                server.read_exact(&mut [0; 12]).unwrap();
                if rejected {
                    server.write_all(&[0, 0, 11, 0, 0, 0, 0, 0]).unwrap();
                } else {
                    assert_eq!(server.read(&mut [0; 1]).unwrap(), 0);
                }
            });
            let start = Instant::now();
            assert!(
                authenticate(
                    client,
                    (vec![], vec![]),
                    0,
                    start + Duration::from_millis(50)
                )
                .is_err()
            );
            assert!(start.elapsed() < Duration::from_secs(1));
            worker.join().unwrap();
        }
    }

    #[test]
    fn blocked_authority_file_open_does_not_delay_fallback_or_publish_late_success() {
        use rustix::fs::{CWD, FileType, Mode};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("authority");
        rustix::fs::mknodat(CWD, &path, FileType::Fifo, Mode::RUSR | Mode::WUSR, 0).unwrap();
        let worker_path = path.clone();
        let (finished, done) = mpsc::channel();
        let start = Instant::now();
        assert!(!bounded_probe(Duration::from_millis(50), move |_, _| {
            let _file = std::fs::File::open(worker_path).unwrap(); // Same blocking open as xauth.
            finished.send(()).unwrap();
            true // Late success must be discarded.
        }));
        assert!(start.elapsed() < Duration::from_secs(1));
        let _writer = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        done.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    #[ignore = "requires an isolated local X11 server"]
    fn authenticated_local_x11_probe_succeeds() {
        assert!(probe_local(None));
    }
}
