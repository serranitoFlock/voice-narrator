//! Tray icon + menu. The menu is rebuilt from `Config::state()` every time
//! that state changes (from the menu, the hotkey, or an external script),
//! which keeps the checkmarks honest without tracking individual items.

use std::sync::Mutex;
use std::time::Duration;

use tauri::image::Image;
use tauri::menu::{
    CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder,
};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use voice_core::config::{Config, Engine, Retention, State, KOKORO_VOICES, RETENTION_OPTIONS};

const TRAY_ID: &str = "main";
const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[cfg(target_os = "macos")]
pub const HOTKEY_LABEL: &str = "⌃⌥⌘V";
#[cfg(target_os = "windows")]
pub const HOTKEY_LABEL: &str = "Ctrl+Alt+Win+V";
#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
pub const HOTKEY_LABEL: &str = "Ctrl+Alt+Super+V";

/// Last state rendered into the tray, to skip rebuilding an unchanged menu
/// (rebuilding would close it if the user has it open).
#[derive(Default)]
pub struct Rendered(Mutex<Option<State>>);

pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let config = app.state::<Config>();
    let state = config.state();
    let menu = build_menu(app, &state)?;

    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| handle_menu_event(app, event.id().as_ref()));
    apply_indicator(builder, state.disabled).build(app)?;

    *app.state::<Rendered>().0.lock().unwrap() = Some(state);
    spawn_poller(app.clone());
    Ok(())
}

/// Re-render the tray if the on-disk state changed since the last render.
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<Config>().state();
    let rendered = app.state::<Rendered>();
    let mut last = rendered.0.lock().unwrap();
    if last.as_ref() == Some(&state) {
        return;
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Ok(menu) = build_menu(app, &state) {
            let _ = tray.set_menu(Some(menu));
        }
        update_indicator(&tray, state.disabled);
    }
    *last = Some(state);
}

pub fn toggle_voice<R: Runtime>(app: &AppHandle<R>) {
    let config = app.state::<Config>();
    let _ = config.set_disabled(!config.is_disabled());
    refresh(app);
}

/// The scripts (and the Swift app) can change the dotfiles at any time, so
/// poll like the original app did instead of relying on file watchers.
fn spawn_poller<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(POLL_INTERVAL);
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || refresh(&handle));
    });
}

fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let config = app.state::<Config>();
    let result = match id.split_once(':') {
        None if id == "toggle" => {
            toggle_voice(app);
            return;
        }
        None if id == "dictionary" => {
            crate::dictionary_window::open(app);
            return;
        }
        None if id == "quit" => {
            app.exit(0);
            return;
        }
        Some(("engine", "kokoro")) => config.set_engine(Engine::Kokoro),
        Some(("engine", "piper")) => config.set_engine(Engine::Piper),
        Some(("kokoro", voice)) => config.set_kokoro_voice(voice),
        Some(("piper", file)) => config.set_piper_voice(file),
        Some(("ret-markers", days)) => set_retention(&config, Retention::Markers, days),
        Some(("ret-log", days)) => set_retention(&config, Retention::Log, days),
        _ => Ok(()),
    };
    if let Err(err) = result {
        eprintln!("voice-toggle: could not apply '{id}': {err}");
    }
    refresh(app);
}

fn set_retention(config: &Config, kind: Retention, days: &str) -> std::io::Result<()> {
    match days.parse() {
        Ok(days) => config.set_retention_days(kind, days),
        Err(_) => Ok(()),
    }
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, s: &State) -> tauri::Result<Menu<R>> {
    let status = MenuItemBuilder::with_id(
        "status",
        if s.disabled { "Voz: desactivada" } else { "Voz: activada" },
    )
    .enabled(false)
    .build(app)?;
    let toggle = MenuItemBuilder::with_id(
        "toggle",
        if s.disabled { "Activar voz" } else { "Desactivar voz" },
    )
    .build(app)?;
    let hotkey = MenuItemBuilder::with_id("hotkey", format!("Atajo global: {HOTKEY_LABEL}"))
        .enabled(false)
        .build(app)?;

    let engine = SubmenuBuilder::new(app, "Motor de voz")
        .item(&check(app, "engine:kokoro", "Kokoro", s.engine == Engine::Kokoro)?)
        .item(&check(app, "engine:piper", "Piper", s.engine == Engine::Piper)?)
        .build()?;

    let mut kokoro = SubmenuBuilder::new(app, "Voz (Kokoro)");
    for (label, code) in KOKORO_VOICES {
        kokoro = kokoro.item(&check(app, &format!("kokoro:{code}"), label, s.kokoro_voice == code)?);
    }

    let mut piper = SubmenuBuilder::new(app, "Voz (Piper)");
    for file in &s.piper_voices {
        let label = file.trim_end_matches(".onnx");
        let selected = s.piper_voice.as_deref() == Some(file.as_str());
        piper = piper.item(&check(app, &format!("piper:{file}"), label, selected)?);
    }

    let dictionary = MenuItemBuilder::with_id("dictionary", "Editar diccionario...").build(app)?;

    let cleanup = SubmenuBuilder::new(app, "Limpieza de archivos")
        .item(&retention_menu(app, "Historial de mensajes", "ret-markers", s.marker_retention_days)?)
        .item(&retention_menu(app, "Registro (hook.log)", "ret-log", s.log_retention_days)?)
        .build()?;

    let quit = MenuItemBuilder::with_id("quit", "Salir").build(app)?;

    MenuBuilder::new(app)
        .item(&status)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&toggle)
        .item(&hotkey)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&engine)
        .item(&kokoro.build()?)
        .item(&piper.build()?)
        .item(&dictionary)
        .item(&cleanup)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&quit)
        .build()
}

fn retention_menu<R: Runtime>(
    app: &AppHandle<R>,
    title: &str,
    prefix: &str,
    selected: u32,
) -> tauri::Result<tauri::menu::Submenu<R>> {
    let mut menu = SubmenuBuilder::new(app, title);
    for days in RETENTION_OPTIONS {
        let label = if days == 1 { "1 día".to_string() } else { format!("{days} días") };
        menu = menu.item(&check(app, &format!("{prefix}:{days}"), &label, days == selected)?);
    }
    menu.build()
}

fn check<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    label: &str,
    checked: bool,
) -> tauri::Result<tauri::menu::CheckMenuItem<R>> {
    CheckMenuItemBuilder::with_id(id, label).checked(checked).build(app)
}

// The macOS menu bar renders text natively, so keep the original emoji
// look there; Windows/Linux trays need an image.
#[cfg(target_os = "macos")]
fn apply_indicator<R: Runtime>(builder: TrayIconBuilder<R>, disabled: bool) -> TrayIconBuilder<R> {
    builder.title(indicator_title(disabled))
}

#[cfg(not(target_os = "macos"))]
fn apply_indicator<R: Runtime>(builder: TrayIconBuilder<R>, disabled: bool) -> TrayIconBuilder<R> {
    builder.icon(indicator_icon(disabled)).tooltip(indicator_title(disabled))
}

#[cfg(target_os = "macos")]
fn update_indicator<R: Runtime>(tray: &tauri::tray::TrayIcon<R>, disabled: bool) {
    let _ = tray.set_title(Some(indicator_title(disabled)));
}

#[cfg(not(target_os = "macos"))]
fn update_indicator<R: Runtime>(tray: &tauri::tray::TrayIcon<R>, disabled: bool) {
    let _ = tray.set_icon(Some(indicator_icon(disabled)));
    let _ = tray.set_tooltip(Some(indicator_title(disabled)));
}

fn indicator_title(disabled: bool) -> &'static str {
    if disabled { "🔇" } else { "🔊" }
}

/// Filled circle: green when the voice is on, grey when it is muted.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn indicator_icon(disabled: bool) -> Image<'static> {
    const SIZE: u32 = 32;
    let color: [u8; 4] = if disabled { [128, 128, 128, 255] } else { [45, 160, 100, 255] };
    let center = SIZE as f32 / 2.0;
    let radius = SIZE as f32 * 0.42;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = ((x as f32 + 0.5 - center).powi(2) + (y as f32 + 0.5 - center).powi(2)).sqrt();
            rgba.extend_from_slice(if d <= radius { &color } else { &[0, 0, 0, 0] });
        }
    }
    Image::new_owned(rgba, SIZE, SIZE)
}
