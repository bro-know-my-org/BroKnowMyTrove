use super::super::X11_TEST_LOCK;
use super::*;
use x11rb::protocol::xtest::ConnectionExt as _;

// Run on an isolated X server: cargo test -p vault-gui x11_copy -- --ignored.
#[test]
#[ignore = "moves the pointer; requires an isolated X11 server with XTEST"]
fn x11_copy_negotiates_uri_proxy_rejection_and_cancellation() {
    let _guard = X11_TEST_LOCK.lock().unwrap();
    let (target_conn, screen) = x11rb::connect(None).unwrap();
    let root = target_conn.setup().roots[screen].root;
    let atoms = Atoms::new(&target_conn).unwrap().reply().unwrap();
    let target = target_conn.generate_id().unwrap();
    target_conn
        .create_window(
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
        .unwrap()
        .check()
        .unwrap();
    target_conn.map_window(target).unwrap();
    let proxy = target_conn.generate_id().unwrap();
    target_conn
        .create_window(
            COPY_DEPTH_FROM_PARENT,
            proxy,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new(),
        )
        .unwrap()
        .check()
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("中文 # %\n.txt");
    std::fs::write(&path, b"authenticated plaintext").unwrap();
    for (use_proxy, accept, cancel, finish, use_root) in [
        (false, true, false, 1, false),
        (true, true, false, 1, false),
        (false, false, false, 1, false),
        (false, true, true, 1, false),
        (false, true, false, 0, false),
        (false, true, false, 2, false),
        (true, true, false, 1, true),
    ] {
        let target = if use_root { root } else { target };
        let (drop_x, drop_y) = if use_root { (1250, 750) } else { (550, 100) };
        target_conn
            .change_property32(
                PropMode::REPLACE,
                target,
                atoms.XdndProxy,
                AtomEnum::WINDOW,
                &[if use_proxy { proxy } else { NONE }],
            )
            .unwrap();
        target_conn
            .change_property32(
                PropMode::REPLACE,
                proxy,
                atoms.XdndProxy,
                AtomEnum::WINDOW,
                &[proxy],
            )
            .unwrap();
        target_conn
            .change_property32(
                PropMode::REPLACE,
                if use_proxy { proxy } else { target },
                atoms.XdndAware,
                AtomEnum::ATOM,
                &[5],
            )
            .unwrap();
        target_conn
            .warp_pointer(NONE, root, 0, 0, 0, 0, 10, 400)
            .unwrap();
        target_conn
            .xtest_fake_input(BUTTON_PRESS_EVENT, 1, 0, root, 0, 0, 0)
            .unwrap();
        target_conn.get_input_focus().unwrap().reply().unwrap();
        let mut drag =
            Source::new(x11rb::connect(None).unwrap().0, root, &path, Arc::default()).unwrap();
        target_conn
            .warp_pointer(NONE, root, 0, 0, 0, 0, drop_x, drop_y)
            .unwrap();
        target_conn.flush().unwrap();
        let mut source = NONE;
        let mut entered = false;
        let mut copied = false;
        let mut dropped = false;
        let mut release = false;
        let mut position_time = 0;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                Instant::now() < deadline,
                "drag did not complete: root={use_root} entered={entered} released={release} dropped={dropped} copied={copied}"
            );
            // Escape after Drop must not abort the target's pending conversion.
            let outcome = drag.poll(
                (cancel && release) || dropped,
                release.then_some((drop_x, drop_y)),
            );
            while let Some(event) = target_conn.poll_for_event().unwrap() {
                match event {
                    Event::ClientMessage(event) if event.type_ == atoms.XdndEnter => {
                        assert_eq!(event.window, target);
                        assert_eq!(event.data.as_data32()[2], atoms.Uri);
                        source = event.data.as_data32()[0];
                        entered = true;
                    }
                    Event::ClientMessage(event) if event.type_ == atoms.XdndPosition => {
                        assert!(entered);
                        assert_eq!(event.data.as_data32()[4], atoms.XdndActionCopy);
                        position_time = event.data.as_data32()[3];
                        std::thread::sleep(Duration::from_millis(25));
                        target_conn
                            .send_event(
                                false,
                                source,
                                EventMask::NO_EVENT,
                                ClientMessageEvent::new(
                                    32,
                                    source,
                                    atoms.XdndStatus,
                                    [target, u32::from(accept), 0, 0, atoms.XdndActionCopy],
                                ),
                            )
                            .unwrap();
                        target_conn
                            .xtest_fake_input(BUTTON_RELEASE_EVENT, 1, 0, root, 0, 0, 0)
                            .unwrap();
                        // Release at the target, then move/repress before the source
                        // polls. The captured release must keep the original target.
                        target_conn
                            .warp_pointer(NONE, root, 0, 0, 0, 0, 10, 400)
                            .unwrap();
                        target_conn
                            .xtest_fake_input(BUTTON_PRESS_EVENT, 1, 0, root, 0, 0, 0)
                            .unwrap();
                        release = true;
                    }
                    Event::ClientMessage(event) if event.type_ == atoms.XdndDrop => {
                        assert!(accept && !cancel);
                        assert!(event.data.as_data32()[2].wrapping_sub(position_time) > 0);
                        dropped = true;
                        target_conn
                            .convert_selection(
                                if use_root { proxy } else { target },
                                atoms.XdndSelection,
                                atoms.Uri,
                                atoms.Uri,
                                event.data.as_data32()[2],
                            )
                            .unwrap();
                    }
                    Event::SelectionNotify(event) => {
                        assert_ne!(event.property, NONE);
                        let bytes = target_conn
                            .get_property(
                                false,
                                event.requestor,
                                event.property,
                                atoms.Uri,
                                0,
                                4096,
                            )
                            .unwrap()
                            .reply()
                            .unwrap()
                            .value;
                        let uri = std::str::from_utf8(&bytes).unwrap();
                        let encoded = uri
                            .strip_prefix("file://")
                            .unwrap()
                            .strip_suffix("\r\n")
                            .unwrap();
                        let decoded = percent_encoding::percent_decode_str(encoded)
                            .decode_utf8()
                            .unwrap();
                        assert_eq!(Path::new(decoded.as_ref()), path);
                        std::fs::copy(decoded.as_ref(), temp.path().join("copy")).unwrap();
                        copied = true;
                        target_conn
                            .send_event(
                                false,
                                source,
                                EventMask::NO_EVENT,
                                ClientMessageEvent::new(
                                    32,
                                    source,
                                    atoms.XdndFinished,
                                    [
                                        target,
                                        u32::from(finish != 0),
                                        if finish == 2 {
                                            NONE
                                        } else {
                                            atoms.XdndActionCopy
                                        },
                                        0,
                                        0,
                                    ],
                                ),
                            )
                            .unwrap();
                    }
                    _ => {}
                }
            }
            target_conn.flush().unwrap();
            if let Some(outcome) = outcome {
                assert_eq!(
                    outcome,
                    if !accept || cancel {
                        2
                    } else if finish == 1 {
                        1
                    } else {
                        4
                    }
                );
                assert_eq!(copied, accept && !cancel);
                assert_eq!(dropped, copied);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(drag);
        target_conn
            .xtest_fake_input(BUTTON_RELEASE_EVENT, 1, 0, root, 0, 0, 0)
            .unwrap();
        assert_eq!(
            target_conn
                .get_selection_owner(atoms.XdndSelection)
                .unwrap()
                .reply()
                .unwrap()
                .owner,
            NONE
        );
        assert!(path.exists());
    }
    target_conn.delete_property(root, atoms.XdndProxy).unwrap();
    target_conn.flush().unwrap();
    let mut disconnected =
        Source::new(x11rb::connect(None).unwrap().0, root, &path, Arc::default()).unwrap();
    rustix::net::shutdown(disconnected.conn.stream(), rustix::net::Shutdown::Both).unwrap();
    assert_eq!(disconnected.poll(false, None), Some(3));
}
