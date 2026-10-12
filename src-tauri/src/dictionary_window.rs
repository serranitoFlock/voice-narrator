//! Dictionary editor window and the commands its page invokes.

use std::fs;

use tauri::{AppHandle, Manager, Runtime, State, WebviewUrl, WebviewWindowBuilder};

use crate::config::Config;
use crate::dictionary::{validate, Dictionary, EntryView};

const WINDOW_LABEL: &str = "dictionary";

pub fn open<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::App("dictionary.html".into()))
        .title("Editar diccionario fonético")
        .inner_size(520.0, 480.0)
        .min_inner_size(420.0, 320.0)
        .center()
        .build();
    match built {
        Ok(window) => {
            let _ = window.set_focus();
        }
        Err(err) => eprintln!("voice-toggle: could not open the dictionary window: {err}"),
    }
}

fn load(config: &Config) -> Result<Dictionary, String> {
    let path = config.dictionary_path();
    let content = fs::read_to_string(&path)
        .map_err(|err| format!("No se pudo leer {}: {err}", path.display()))?;
    let mut dict = Dictionary::parse(&content);
    dict.sort();
    Ok(dict)
}

fn store(config: &Config, mut dict: Dictionary) -> Result<Vec<EntryView>, String> {
    dict.sort();
    let path = config.dictionary_path();
    fs::write(&path, dict.serialize())
        .map_err(|err| format!("No se pudo guardar el diccionario: {err}"))?;
    Ok(dict.views())
}

#[tauri::command]
pub fn dictionary_list(config: State<'_, Config>) -> Result<Vec<EntryView>, String> {
    Ok(load(&config)?.views())
}

/// Adds a word, or edits `original` (renaming it when the word changed).
#[tauri::command]
pub fn dictionary_save(
    config: State<'_, Config>,
    original: Option<String>,
    word: String,
    respelling: String,
) -> Result<Vec<EntryView>, String> {
    let (word, respelling) = (word.trim(), respelling.trim());
    validate(word, respelling)?;
    let mut dict = load(&config)?;
    match original {
        Some(original) => dict.edit(&original, word, respelling),
        None => dict.upsert(word, respelling),
    }
    store(&config, dict)
}

#[tauri::command]
pub fn dictionary_delete(config: State<'_, Config>, word: String) -> Result<Vec<EntryView>, String> {
    let mut dict = load(&config)?;
    dict.delete(&word);
    store(&config, dict)
}
