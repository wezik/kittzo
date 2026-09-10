use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

const ICON_BYTES: &[u8] = include_bytes!("../../resources/icons/full_32.png");

pub fn spawn(server_port: u16, paused: Arc<AtomicBool>) -> ! {
    let event_loop = EventLoopBuilder::<MenuEvent>::with_user_event().build();

    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(event);
    }));

    let menu = Menu::new();
    let open_ui = MenuItem::with_id("open-ui", "Open UI", true, None);
    let pause_resume = MenuItem::with_id("pause-resume", "Pause", true, None);
    let quit = MenuItem::with_id("quit", "Quit", true, None);
    menu.append_items(&[&open_ui, &pause_resume, &quit])
        .expect("failed to build tray menu");

    let mut icon = Some(load_icon());
    let mut tray_icon = None;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                tray_icon = Some(
                    TrayIconBuilder::new()
                        .with_menu(Box::new(menu.clone()))
                        .with_tooltip("kittzo")
                        .with_icon(icon.take().expect("icon already taken"))
                        .build()
                        .expect("failed to build tray icon"),
                );
            }
            Event::UserEvent(event) if event.id == "open-ui" => {
                open_browser(server_port);
            }
            Event::UserEvent(event) if event.id == "pause-resume" => {
                let now_paused = !paused.load(Ordering::SeqCst);
                paused.store(now_paused, Ordering::SeqCst);
                pause_resume.set_text(if now_paused { "Resume" } else { "Pause" });
            }
            Event::UserEvent(event) if event.id == "quit" => {
                tray_icon.take();
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    })
}

fn load_icon() -> Icon {
    let image = image::load_from_memory(ICON_BYTES)
        .expect("invalid tray icon bytes")
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).expect("invalid tray icon data")
}

#[cfg(target_os = "windows")]
fn open_browser(port: u16) {
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", &format!("http://127.0.0.1:{port}")])
        .spawn();
}

#[cfg(target_os = "linux")]
fn open_browser(port: u16) {
    let _ = std::process::Command::new("xdg-open")
        .arg(format!("http://127.0.0.1:{port}"))
        .spawn();
}
