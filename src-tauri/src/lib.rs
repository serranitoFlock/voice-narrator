pub mod config;
pub mod dictionary;
mod dictionary_window;
mod tray;

use tauri::RunEvent;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use config::Config;

/// Control+Option+Command+V on macOS, Control+Alt+Win/Super+V elsewhere.
fn toggle_shortcut() -> Shortcut {
    Shortcut::new(
        Some(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER),
        Code::KeyV,
    )
}

fn pipeline_root() -> std::path::PathBuf {
    std::env::home_dir()
        .expect("could not resolve the home directory")
        .join("voice-pipeline")
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if *shortcut == toggle_shortcut() && event.state() == ShortcutState::Pressed {
                        tray::toggle_voice(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            dictionary_window::dictionary_list,
            dictionary_window::dictionary_save,
            dictionary_window::dictionary_delete,
        ])
        .manage(Config::new(pipeline_root()))
        .manage(tray::Rendered::default())
        .setup(|app| {
            // Menu-bar only: no Dock icon.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            tray::create(app.handle())?;

            // A taken shortcut must not keep the tray from starting.
            if let Err(err) = app.global_shortcut().register(toggle_shortcut()) {
                eprintln!("voice-toggle: could not register the global shortcut: {err}");
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the tray app");

    app.run(|_app, event| {
        // Tray-only app: closing a future window must not quit it; only
        // "Salir" (which exits with an explicit code) does.
        if let RunEvent::ExitRequested { api, code: None, .. } = event {
            api.prevent_exit();
        }
    });
}
