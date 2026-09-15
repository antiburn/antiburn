#[cfg(target_os = "macos")]
fn main() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_foundation::NSPoint;
    use tray_icon::{MouseButtonState, TrayIconBuilder, TrayIconEvent};

    if std::env::var_os("TRAY_ICON_RUN_NATIVE_TEST").is_none() {
        println!("native AppKit test skipped; set TRAY_ICON_RUN_NATIVE_TEST=1 to run it");
        return;
    }

    let mtm = MainThreadMarker::new().expect("the custom test harness runs on the main thread");
    let _app = NSApplication::sharedApplication(mtm);
    let tray = TrayIconBuilder::new()
        .build()
        .expect("create the status item");

    tray.set_highlight_override(Some(false));
    assert!(!is_highlighted(&tray, mtm));
    dispatch_primary_event(&tray, mtm, NSEventType::LeftMouseDown);
    assert!(!is_highlighted(&tray, mtm));
    assert_click(MouseButtonState::Down);
    dispatch_primary_event(&tray, mtm, NSEventType::LeftMouseUp);
    assert!(!is_highlighted(&tray, mtm));
    assert_click(MouseButtonState::Up);

    tray.set_highlight_override(Some(true));
    assert!(is_highlighted(&tray, mtm));
    dispatch_primary_event(&tray, mtm, NSEventType::LeftMouseDown);
    dispatch_primary_event(&tray, mtm, NSEventType::LeftMouseUp);
    assert!(is_highlighted(&tray, mtm));
    assert_click(MouseButtonState::Down);
    assert_click(MouseButtonState::Up);

    tray.set_highlight_override(None);
    assert!(!is_highlighted(&tray, mtm));
    dispatch_primary_event(&tray, mtm, NSEventType::LeftMouseDown);
    assert!(is_highlighted(&tray, mtm));
    assert_click(MouseButtonState::Down);
    dispatch_primary_event(&tray, mtm, NSEventType::LeftMouseUp);
    assert!(!is_highlighted(&tray, mtm));
    assert_click(MouseButtonState::Up);

    tray.set_highlight_override(Some(true));
    tray.set_visible(false).expect("hide the status item");
    tray.set_visible(true).expect("recreate the status item");
    assert!(is_highlighted(&tray, mtm));

    println!("native AppKit highlight override checks passed");

    fn button(
        tray: &tray_icon::TrayIcon,
        mtm: MainThreadMarker,
    ) -> objc2::rc::Retained<objc2_app_kit::NSStatusBarButton> {
        tray.ns_status_item()
            .expect("the status item is visible")
            .button(mtm)
            .expect("the status item has a button")
    }

    fn is_highlighted(tray: &tray_icon::TrayIcon, mtm: MainThreadMarker) -> bool {
        button(tray, mtm).isHighlighted()
    }

    fn dispatch_primary_event(
        tray: &tray_icon::TrayIcon,
        mtm: MainThreadMarker,
        event_type: NSEventType,
    ) {
        let button = button(tray, mtm);

        let window = button.window().expect("the status button has a window");
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                event_type,
                NSPoint::new(0.0, 0.0),
                NSEventModifierFlags::empty(),
                0.0,
                window.windowNumber(),
                None,
                1,
                1,
                1.0,
            )
            .expect("create a native mouse event");
        let target = button
            .subviews()
            .lastObject()
            .expect("the status button has the tray event target");

        match event_type {
            NSEventType::LeftMouseDown => target.mouseDown(&event),
            NSEventType::LeftMouseUp => target.mouseUp(&event),
            _ => panic!("only primary mouse events are supported"),
        }
    }

    fn assert_click(expected: MouseButtonState) {
        let event = TrayIconEvent::receiver()
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("the native handler delivers its click event");
        match event {
            TrayIconEvent::Click { button_state, .. } => assert_eq!(button_state, expected),
            _ => panic!("expected a tray click event"),
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("native AppKit test skipped on this platform");
}
