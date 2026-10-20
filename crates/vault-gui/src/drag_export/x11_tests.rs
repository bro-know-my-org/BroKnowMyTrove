use super::*;
use std::sync::Arc;
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, NONE};
use x11rb::{
    connection::Connection,
    protocol::{Event, xproto::*},
    wrapper::ConnectionExt as _,
};

#[test]
fn capability_rejects_tcp_and_ssh_forwarded_displays() {
    assert!(local_display(Some(":1")));
    assert!(local_display(Some("unix/:1")));
    assert!(!local_display(Some("localhost:10.0")));
    assert!(!local_display(Some("192.0.2.1:0")));
}

#[test]
fn raw_release_uses_window_origin_and_gui_scaling() {
    use eframe::egui;
    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(2.0);
    let mut output = ctx.run_ui(Default::default(), |_| {});
    output.textures_delta.clear();
    assert_eq!(ctx.pixels_per_point(), 2.0);
    let (sender, receiver) = mpsc::channel();
    let drag = Drag {
        control: Arc::new(Control::new()),
        sender,
        outcome: Arc::default(),
    };
    let mut raw = egui::RawInput::default();
    raw.viewports.get_mut(&raw.viewport_id).unwrap().inner_rect = Some(egui::Rect::from_min_size(
        egui::pos2(100.0, 200.0),
        egui::vec2(500.0, 400.0),
    ));
    raw.events.push(egui::Event::PointerButton {
        pos: egui::pos2(30.0, 40.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    drag.input(&ctx, &raw);
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Input::Release(260, 480)
    ));
    drag.control.completed.store(true, Ordering::SeqCst);
}

#[test]
fn stalled_setup_is_interrupted_and_worker_terminates() {
    use std::{
        io::Read,
        os::unix::net::{UnixListener, UnixStream},
    };
    for committed in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let listener = UnixListener::bind(temp.path().join("server")).unwrap();
        let client = UnixStream::connect(temp.path().join("server")).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let control = Arc::new(Control::new());
        control.phase.store(u8::from(committed), Ordering::SeqCst);
        let worker_control = control.clone();
        let (done, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let stream = DefaultStream::from_unix_stream(client).unwrap().0;
            done.send(
                worker_control
                    .handshake(stream, 0, (Vec::new(), Vec::new()))
                    .is_err(),
            )
            .unwrap();
        });
        server.read_exact(&mut [0; 12]).unwrap(); // Accept setup, never reply.
        let (sender, _) = mpsc::channel();
        let outcome = Arc::new(AtomicU8::new(0));
        let drag = Drag {
            control,
            sender,
            outcome: outcome.clone(),
        };
        *drag.control.progress.lock().unwrap() = Instant::now() - Duration::from_secs(4);
        drag.poll();
        assert_eq!(
            outcome.load(Ordering::SeqCst),
            if committed { 4 } else { 3 }
        );
        assert!(result.recv_timeout(Duration::from_secs(1)).unwrap());
        worker.join().unwrap();
    }
}

#[test]
#[ignore = "requires an isolated X11 server with XTEST"]
fn x11_worker_lock_sends_leave_cleans_plaintext_and_isolates_late_result() {
    let _guard = X11_TEST_LOCK.lock().unwrap();
    let (conn, screen) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen].root;
    let atoms = Atoms::new(&conn).unwrap().reply().unwrap();
    let target = conn.generate_id().unwrap();
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        target,
        root,
        500,
        0,
        300,
        300,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new(),
    )
    .unwrap();
    conn.change_property32(
        PropMode::REPLACE,
        target,
        atoms.XdndAware,
        AtomEnum::ATOM,
        &[5],
    )
    .unwrap();
    conn.map_window(target).unwrap();
    conn.warp_pointer(NONE, root, 0, 0, 0, 0, 550, 100).unwrap();
    conn.xtest_fake_input(BUTTON_PRESS_EVENT, 1, 0, root, 0, 0, 0)
        .unwrap();
    conn.get_input_focus().unwrap().reply().unwrap();
    let ctx = eframe::egui::Context::default();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("plain.txt");
    std::fs::write(&path, b"plaintext").unwrap();
    let mut exports = super::super::Exports::default();
    let old_outcome = exports.outcome.clone();
    exports.active = Some(super::super::Prepared {
        directory,
        path: path.clone(),
    });
    exports.native =
        Some(Drag::start_window(root, &path, old_outcome.clone(), ctx.clone()).unwrap());
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut locked = false;
    let mut left = false;
    while !left || old_outcome.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "worker failed to cancel");
        while let Some(event) = conn.poll_for_event().unwrap() {
            if let Event::ClientMessage(event) = event {
                if event.type_ == atoms.XdndEnter && !locked {
                    exports.lock();
                    exports.poll(&ctx);
                    locked = true;
                    assert!(!path.exists());
                } else if event.type_ == atoms.XdndLeave {
                    left = true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(exports.outcome.load(Ordering::SeqCst), 0);
    assert_eq!(
        conn.get_selection_owner(atoms.XdndSelection)
            .unwrap()
            .reply()
            .unwrap()
            .owner,
        NONE
    );
    // A replacement worker must finish its committed selection even when Escape
    // arrives before the receiver has asked for the URI.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("committed.txt");
    std::fs::write(&path, b"committed plaintext").unwrap();
    exports.active = Some(super::super::Prepared {
        directory,
        path: path.clone(),
    });
    exports.native =
        Some(Drag::start_window(root, &path, exports.outcome.clone(), ctx.clone()).unwrap());
    let mut raw = eframe::egui::RawInput::default();
    raw.viewports.get_mut(&raw.viewport_id).unwrap().inner_rect =
        Some(eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            eframe::egui::vec2(1280.0, 800.0),
        ));
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut source = NONE;
    while exports.outcome.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < deadline,
            "committed worker failed to finish"
        );
        while let Some(event) = conn.poll_for_event().unwrap() {
            match event {
                Event::ClientMessage(event) if event.type_ == atoms.XdndEnter => {
                    source = event.data.as_data32()[0]
                }
                Event::ClientMessage(event) if event.type_ == atoms.XdndPosition => {
                    conn.send_event(
                        false,
                        source,
                        EventMask::NO_EVENT,
                        ClientMessageEvent::new(
                            32,
                            source,
                            atoms.XdndStatus,
                            [target, 1, 0, 0, atoms.XdndActionCopy],
                        ),
                    )
                    .unwrap();
                    raw.events = vec![eframe::egui::Event::PointerButton {
                        pos: eframe::egui::pos2(550.0, 100.0),
                        button: eframe::egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: eframe::egui::Modifiers::NONE,
                    }];
                    exports.input(&ctx, &raw);
                }
                Event::ClientMessage(event) if event.type_ == atoms.XdndDrop => {
                    raw.events = vec![eframe::egui::Event::Key {
                        key: eframe::egui::Key::Escape,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: eframe::egui::Modifiers::NONE,
                    }];
                    exports.input(&ctx, &raw);
                    assert!(
                        !exports
                            .native
                            .as_ref()
                            .unwrap()
                            .control
                            .canceled
                            .load(Ordering::SeqCst)
                    );
                    conn.convert_selection(
                        target,
                        atoms.XdndSelection,
                        atoms.Uri,
                        atoms.Uri,
                        event.data.as_data32()[2],
                    )
                    .unwrap();
                }
                Event::SelectionNotify(event) => {
                    assert_ne!(event.property, NONE);
                    assert_eq!(std::fs::read(&path).unwrap(), b"committed plaintext");
                    conn.send_event(
                        false,
                        source,
                        EventMask::NO_EVENT,
                        ClientMessageEvent::new(
                            32,
                            source,
                            atoms.XdndFinished,
                            [target, 1, atoms.XdndActionCopy, 0, 0],
                        ),
                    )
                    .unwrap();
                }
                _ => {}
            }
        }
        conn.flush().unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(exports.outcome.load(Ordering::SeqCst), 1);
    exports.poll(&ctx);
    assert!(path.exists());
    exports.lock();
    exports.poll(&ctx);
    assert!(!path.exists());
    conn.xtest_fake_input(BUTTON_RELEASE_EVENT, 1, 0, root, 0, 0, 0)
        .unwrap();
    conn.flush().unwrap();
}
