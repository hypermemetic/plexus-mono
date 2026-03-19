#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// cocoa crate is deprecated in favor of objc2-* — suppress until migration
#![allow(deprecated)]
#![allow(unexpected_cfgs)]

#[macro_use]
extern crate objc;

use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

#[cfg(target_os = "macos")]
fn set_activation_policy() {
    use cocoa::appkit::{NSApp, NSApplication, NSApplicationActivationPolicy};
    unsafe {
        let app = NSApp();
        app.setActivationPolicy_(
            NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn set_activation_policy() {}

/// Register a proper NSPanel subclass at runtime with canBecomeKeyWindow override.
/// object_setClass alone doesn't allow method overrides — we need ClassDecl.
#[cfg(target_os = "macos")]
fn register_panel_class() -> &'static objc::runtime::Class {
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel, BOOL, YES};

    static INIT: std::sync::Once = std::sync::Once::new();
    static mut PANEL_CLASS: Option<&'static Class> = None;

    extern "C" fn can_become_key(_this: &Object, _sel: Sel) -> BOOL {
        YES
    }

    extern "C" fn can_become_main(_this: &Object, _sel: Sel) -> BOOL {
        YES
    }

    unsafe {
        INIT.call_once(|| {
            let superclass = Class::get("NSPanel").unwrap();
            let mut decl = ClassDecl::new("MonoTrayPanel", superclass).unwrap();
            decl.add_method(
                sel!(canBecomeKeyWindow),
                can_become_key as extern "C" fn(&Object, Sel) -> BOOL,
            );
            decl.add_method(
                sel!(canBecomeMainWindow),
                can_become_main as extern "C" fn(&Object, Sel) -> BOOL,
            );
            PANEL_CLASS = Some(decl.register());
        });
        PANEL_CLASS.unwrap()
    }
}

/// Convert the Tauri NSWindow into a MonoTrayPanel (NSPanel subclass).
#[cfg(target_os = "macos")]
fn configure_as_panel(window: &tauri::WebviewWindow) {
    use cocoa::appkit::{NSWindow, NSWindowCollectionBehavior};
    use cocoa::base::id;
    use cocoa::foundation::NSString;
    use std::ffi::c_void;

    extern "C" {
        fn object_setClass(obj: *mut c_void, cls: *const c_void) -> *const c_void;
    }

    let ns_window: id = window.ns_window().unwrap() as id;
    unsafe {
        // Swap class from NSWindow → MonoTrayPanel (proper subclass with overrides)
        let panel_class = register_panel_class();
        object_setClass(
            ns_window as *mut c_void,
            panel_class as *const _ as *const c_void,
        );

        // NSNonactivatingPanelMask (1 << 7) — panel doesn't activate the app
        let current_mask: u64 = msg_send![ns_window, styleMask];
        let new_mask = current_mask | (1u64 << 7);
        let _: () = msg_send![ns_window, setStyleMask: new_mask];

        // Collection behavior for fullscreen overlay
        ns_window.setCollectionBehavior_(
            NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
                | NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary
                | NSWindowCollectionBehavior::NSWindowCollectionBehaviorIgnoresCycle
                | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary,
        );

        // Panel-specific properties
        let _: () = msg_send![ns_window, setHidesOnDeactivate: false];
        let _: () = msg_send![ns_window, setFloatingPanel: true];
        let _: () = msg_send![ns_window, setWorksWhenModal: true];
        let _: () = msg_send![ns_window, setBecomesKeyOnlyIfNeeded: false];
        let _: () = msg_send![ns_window, setIgnoresMouseEvents: false];

        // Transparent window — no native background, no native shadow
        let _: () = msg_send![ns_window, setOpaque: false];
        let clear_color: id =
            msg_send![objc::runtime::Class::get("NSColor").unwrap(), clearColor];
        let _: () = msg_send![ns_window, setBackgroundColor: clear_color];
        let _: () = msg_send![ns_window, setHasShadow: false];

        // Force WKWebView to be transparent (re-apply after class swap)
        let content_view: id = msg_send![ns_window, contentView];
        let subviews: id = msg_send![content_view, subviews];
        let count: usize = msg_send![subviews, count];
        let no: id = msg_send![objc::runtime::Class::get("NSNumber").unwrap(), numberWithBool: false];
        let key: id = cocoa::foundation::NSString::alloc(cocoa::base::nil)
            .init_str("drawsBackground");
        for i in 0..count {
            let subview: id = msg_send![subviews, objectAtIndex: i];
            let sel = objc::runtime::Sel::register("setValue:forKey:");
            let responds: bool = msg_send![subview, respondsToSelector: sel];
            if responds {
                let _: () = msg_send![subview, setValue: no forKey: key];
            }
        }

        // Set window level LAST — Chromium checks window.level > NSNormalWindowLevel
        // to decide whether to deliver mouseMoved to unfocused windows.
        // Setting it after all other config ensures it sticks.
        ns_window.setLevel_(25); // NSStatusWindowLevel
    }
}

#[cfg(not(target_os = "macos"))]
fn configure_as_panel(_window: &tauri::WebviewWindow) {}

/// Install a global mouse-click monitor that hides the window when
/// the user clicks anywhere outside it.
#[cfg(target_os = "macos")]
fn install_click_outside_monitor(window: tauri::WebviewWindow) {
    use cocoa::base::id;
    use cocoa::foundation::NSRect;
    use std::sync::Arc;

    let win = Arc::new(window);
    let win_clone = win.clone();

    let block = block::ConcreteBlock::new(move |_event: id| {
        if let Some(ns_win) = win_clone.ns_window().ok().map(|w| w as id) {
            unsafe {
                let visible: bool = msg_send![ns_win, isVisible];
                if visible {
                    let mouse_loc: cocoa::foundation::NSPoint =
                        msg_send![objc::runtime::Class::get("NSEvent").unwrap(), mouseLocation];
                    let frame: NSRect = msg_send![ns_win, frame];
                    let inside = mouse_loc.x >= frame.origin.x
                        && mouse_loc.x <= frame.origin.x + frame.size.width
                        && mouse_loc.y >= frame.origin.y
                        && mouse_loc.y <= frame.origin.y + frame.size.height;
                    if !inside {
                        let _ = win_clone.emit("mono-tray://hide", ());
                    }
                }
            }
        }
    });
    let block = block.copy();

    unsafe {
        let ns_event_class = objc::runtime::Class::get("NSEvent").unwrap();
        // NSLeftMouseDownMask | NSRightMouseDownMask
        let mask: u64 = (1 << 1) | (1 << 3);
        let _: id = msg_send![ns_event_class, addGlobalMonitorForEventsMatchingMask:mask handler:&*block];
        std::mem::forget(block);
    }
}

#[cfg(not(target_os = "macos"))]
fn install_click_outside_monitor(_window: tauri::WebviewWindow) {}

/// Install a global mouse-move monitor that forwards cursor position to JS
/// when the cursor is inside our panel. This bypasses WebKit's broken
/// mouseMoved delivery in non-activating panels.
#[cfg(target_os = "macos")]
fn install_mouse_move_monitor(window: tauri::WebviewWindow) {
    use cocoa::base::id;
    use cocoa::foundation::NSRect;
    use std::sync::Arc;

    let win = Arc::new(window);
    let win_clone = win.clone();

    let block = block::ConcreteBlock::new(move |_event: id| {
        if let Some(ns_win) = win_clone.ns_window().ok().map(|w| w as id) {
            unsafe {
                let visible: bool = msg_send![ns_win, isVisible];
                if !visible {
                    return;
                }
                let mouse_loc: cocoa::foundation::NSPoint =
                    msg_send![objc::runtime::Class::get("NSEvent").unwrap(), mouseLocation];
                let frame: NSRect = msg_send![ns_win, frame];
                let inside = mouse_loc.x >= frame.origin.x
                    && mouse_loc.x <= frame.origin.x + frame.size.width
                    && mouse_loc.y >= frame.origin.y
                    && mouse_loc.y <= frame.origin.y + frame.size.height;
                if inside {
                    // Convert to window-local coordinates (origin top-left for web)
                    let local_x = mouse_loc.x - frame.origin.x;
                    let local_y = frame.size.height - (mouse_loc.y - frame.origin.y);
                    let _ = win_clone.emit(
                        "mono-tray://mousemove",
                        serde_json::json!({ "x": local_x, "y": local_y }),
                    );
                } else {
                    let _ = win_clone.emit("mono-tray://mouseleave", ());
                }
            }
        }
    });
    let block = block.copy();

    unsafe {
        let ns_event_class = objc::runtime::Class::get("NSEvent").unwrap();
        // NSMouseMovedMask (1 << 5)
        let mask: u64 = 1 << 5;
        let _: id = msg_send![ns_event_class, addGlobalMonitorForEventsMatchingMask:mask handler:&*block];
        std::mem::forget(block);
    }
}

#[cfg(not(target_os = "macos"))]
fn install_mouse_move_monitor(_window: tauri::WebviewWindow) {}

/// Hide the panel when the active macOS Space changes (e.g. swipe, Mission Control).
#[cfg(target_os = "macos")]
fn install_space_change_monitor(window: tauri::WebviewWindow) {
    use cocoa::base::id;
    use cocoa::foundation::NSString;
    use std::sync::Arc;

    let win = Arc::new(window);

    let block = block::ConcreteBlock::new(move |_notif: id| {
        let _ = win.emit("mono-tray://hide", ());
    });
    let block = block.copy();

    unsafe {
        let workspace: id = msg_send![
            objc::runtime::Class::get("NSWorkspace").unwrap(),
            sharedWorkspace
        ];
        let center: id = msg_send![workspace, notificationCenter];
        let name: id = cocoa::foundation::NSString::alloc(cocoa::base::nil)
            .init_str("NSWorkspaceActiveSpaceDidChangeNotification");
        let _: id = msg_send![center,
            addObserverForName: name
            object: cocoa::base::nil
            queue: cocoa::base::nil
            usingBlock: &*block
        ];
        std::mem::forget(block);
    }
}

#[cfg(not(target_os = "macos"))]
fn install_space_change_monitor(_window: tauri::WebviewWindow) {}

fn show_window(app: &tauri::AppHandle, tray_rect: Option<tauri::Rect>) {
    if let Some(window) = app.get_webview_window("main") {
        let visible = window.is_visible().unwrap_or(false);
        if visible {
            let _ = app.emit("mono-tray://hide", ());
        } else {
            if let Some(rect) = tray_rect {
                let scale = window.scale_factor().unwrap_or(2.0);
                let pos = rect.position.to_logical::<f64>(scale);
                let size = rect.size.to_logical::<f64>(scale);
                let x = pos.x - 176.0 + (size.width / 2.0);
                let y = pos.y + size.height + 4.0;
                let _ = window.set_position(tauri::Position::Logical(
                    tauri::LogicalPosition::new(x, y),
                ));
            }

            let _ = window.show();
            let _ = window.set_focus();

            // Explicitly make key window + first responder on the WKWebView
            #[cfg(target_os = "macos")]
            {
                use cocoa::base::id;
                let ns_win: id = window.ns_window().unwrap() as id;
                unsafe {
                    let _: () = msg_send![ns_win, makeKeyAndOrderFront: cocoa::base::nil];
                    // Make WKWebView first responder so it receives mouse events
                    let content_view: id = msg_send![ns_win, contentView];
                    let subviews: id = msg_send![content_view, subviews];
                    let count: usize = msg_send![subviews, count];
                    if count > 0 {
                        let webview: id = msg_send![subviews, objectAtIndex: 0u64];
                        let _: () = msg_send![ns_win, makeFirstResponder: webview];
                    }
                }
            }

            let _ = app.emit("mono-tray://show", ());
        }
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            set_activation_policy();

            if let Some(window) = app.get_webview_window("main") {
                configure_as_panel(&window);
                install_space_change_monitor(window.clone());
                install_click_outside_monitor(window.clone());
                install_mouse_move_monitor(window);
            }

            // Right-click context menu
            let open_music = MenuItemBuilder::with_id("open_music", "Open Music Folder").build(app)?;
            let restart_server = MenuItemBuilder::with_id("restart_server", "Restart Server").build(app)?;
            let restart_app = MenuItemBuilder::with_id("restart_app", "Restart").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&open_music)
                .item(&restart_server)
                .separator()
                .item(&restart_app)
                .item(&quit)
                .build()?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .icon_as_template(true)
                .tooltip("Mono Tray")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    match event.id().as_ref() {
                        "open_music" => {
                            let music_dir = dirs::home_dir()
                                .unwrap_or_default()
                                .join("Music/mono-tray");
                            let _ = std::process::Command::new("open").arg(music_dir).spawn();
                        }
                        "restart_server" => {
                            // Kill backend on port 4448, then relaunch
                            let _ = std::process::Command::new("sh")
                                .args(["-c", "lsof -ti :4448 | xargs kill 2>/dev/null"])
                                .spawn();
                            // Give it a moment to die, then relaunch
                            let handle = app.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(500));
                                // Try the installed binary first; fall back to cargo run in mono-provider
                                let spawned = std::process::Command::new("plexus-mono").spawn();
                                if spawned.is_err() {
                                    let mono_provider_dir = std::env::current_exe()
                                        .ok()
                                        .and_then(|p| {
                                            // Walk up from the .app bundle to find mono-provider/
                                            let mut dir = p.parent()?.to_path_buf();
                                            while dir.pop() {
                                                if dir.join("mono-provider/Cargo.toml").exists() {
                                                    return Some(dir.join("mono-provider"));
                                                }
                                            }
                                            None
                                        })
                                        .unwrap_or_else(|| {
                                            dirs::home_dir()
                                                .unwrap_or_default()
                                                .join("dev/controlflow/hypermemetic/mono-provider")
                                        });
                                    let _ = std::process::Command::new("cargo")
                                        .args(["run", "--release"])
                                        .current_dir(mono_provider_dir)
                                        .spawn();
                                }
                                // Notify frontend to reconnect
                                let _ = handle.emit("mono-tray://show", ());
                            });
                        }
                        "restart_app" => {
                            app.restart();
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        show_window(tray.app_handle(), Some(rect));
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
