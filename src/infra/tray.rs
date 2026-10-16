use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::app::payment;
use crate::app::startup_task::StartupTask;

const ICON_BYTES: &[u8] = include_bytes!("../../resources/icons/kittzo2_32.png");
const BELL_ICON_BYTES: &[u8] = include_bytes!("../../resources/icons/kittzo2_bell_32.png");
const PENDING_ACK_POLL: Duration = Duration::from_secs(10);

pub struct PendingAckWatch {
    payment_service: Arc<payment::Service>,
    pending_ack: Arc<AtomicBool>,
}

impl PendingAckWatch {
    pub fn new(payment_service: Arc<payment::Service>, pending_ack: Arc<AtomicBool>) -> Self {
        Self {
            payment_service,
            pending_ack,
        }
    }
}

#[async_trait]
impl StartupTask for PendingAckWatch {
    fn name(&self) -> &str {
        "pending-ack-watch"
    }

    async fn run(&self) {
        loop {
            if let Ok(pending) = self.payment_service.has_pending_ack().await {
                self.pending_ack.store(pending, Ordering::Relaxed);
            }
            tokio::time::sleep(PENDING_ACK_POLL).await;
        }
    }
}

pub fn spawn(server_port: u16, paused: Arc<AtomicBool>, pending_ack: Arc<AtomicBool>) -> ! {
    let event_loop = EventLoopBuilder::<MenuEvent>::with_user_event().build();

    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(event);
    }));

    let menu = Menu::new();
    let open_ui = MenuItem::with_id("open-ui", "Open UI", true, None);
    let pay = MenuItem::with_id(
        "pay",
        "Run payment script",
        cfg!(target_os = "windows"),
        None,
    );
    let pause_resume = MenuItem::with_id("pause-resume", "Pause", true, None);
    let quit = MenuItem::with_id("quit", "Quit", true, None);
    menu.append_items(&[&open_ui, &pay, &pause_resume, &quit])
        .expect("failed to build tray menu");

    let mut icon = Some(load_icon(ICON_BYTES));
    let mut tray_icon: Option<TrayIcon> = None;
    let mut showing_bell = false;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1));

        let pending = pending_ack.load(Ordering::Relaxed);
        if pending != showing_bell
            && let Some(tray_icon) = &tray_icon
        {
            let bytes = if pending { BELL_ICON_BYTES } else { ICON_BYTES };
            let _ = tray_icon.set_icon(Some(load_icon(bytes)));
            showing_bell = pending;
        }

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
            Event::UserEvent(event) if event.id == "pay" => {
                open_payment_scripts();
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

fn load_icon(bytes: &[u8]) -> Icon {
    let image = image::load_from_memory(bytes)
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

#[cfg(target_os = "windows")]
fn open_payment_scripts() {
    let _ = std::process::Command::new("cmd")
        .args([
            "/C",
            "start",
            "kittzo - payment scripts",
            "/D",
            "scripts",
            "cmd",
            "/K",
            r".venv\Scripts\python.exe",
            "run.py",
        ])
        .spawn();
}

#[cfg(target_os = "linux")]
fn open_payment_scripts() {}

#[cfg(target_os = "linux")]
fn open_browser(port: u16) {
    let _ = std::process::Command::new("xdg-open")
        .arg(format!("http://127.0.0.1:{port}"))
        .spawn();
}
