use crate::dictionary::{edit_monitor, DictionaryEntry, DictionaryManager, DictionaryStatus};
use crate::settings::{get_settings, write_settings};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
#[specta::specta]
pub fn list_dictionary(
    dictionary: State<'_, Arc<DictionaryManager>>,
) -> Result<Vec<DictionaryEntry>, String> {
    dictionary.list().map_err(|e| e.to_string())
}

/// Adds words the user typed; returns the ones that were new.
#[tauri::command]
#[specta::specta]
pub fn add_dictionary_words(
    dictionary: State<'_, Arc<DictionaryManager>>,
    words: Vec<String>,
) -> Result<Vec<String>, String> {
    dictionary.add_manual(&words).map_err(|e| e.to_string())
}

/// Adds the words of a pasted list (commas, semicolons or newlines).
#[tauri::command]
#[specta::specta]
pub fn import_dictionary_text(
    dictionary: State<'_, Arc<DictionaryManager>>,
    text: String,
) -> Result<Vec<String>, String> {
    dictionary.import_text(&text).map_err(|e| e.to_string())
}

/// The active words, one per line.
#[tauri::command]
#[specta::specta]
pub fn export_dictionary_text(
    dictionary: State<'_, Arc<DictionaryManager>>,
) -> Result<String, String> {
    dictionary.export_text().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn rename_dictionary_entry(
    dictionary: State<'_, Arc<DictionaryManager>>,
    id: i64,
    word: String,
) -> Result<DictionaryEntry, String> {
    dictionary.rename(id, &word).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn remove_dictionary_entry(
    dictionary: State<'_, Arc<DictionaryManager>>,
    id: i64,
) -> Result<(), String> {
    dictionary.remove(id).map_err(|e| e.to_string())
}

/// Accepts (`active`) or dismisses a suggestion, or puts a word back under
/// review.
#[tauri::command]
#[specta::specta]
pub fn set_dictionary_entry_status(
    dictionary: State<'_, Arc<DictionaryManager>>,
    id: i64,
    status: DictionaryStatus,
) -> Result<(), String> {
    dictionary.set_status(id, status).map_err(|e| e.to_string())
}

/// Accepts or dismisses every pending suggestion; returns how many.
#[tauri::command]
#[specta::specta]
pub fn resolve_pending_dictionary_entries(
    dictionary: State<'_, Arc<DictionaryManager>>,
    status: DictionaryStatus,
) -> Result<u32, String> {
    dictionary
        .resolve_pending(status)
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn clear_dictionary(dictionary: State<'_, Arc<DictionaryManager>>) -> Result<(), String> {
    dictionary.clear().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn is_dictionary_edit_learning_supported() -> bool {
    edit_monitor::supported()
}

fn settings_changed(app: &AppHandle, setting: &str, value: bool) {
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({ "setting": setting, "value": value }),
    );
}

#[tauri::command]
#[specta::specta]
pub fn change_dictionary_auto_accept_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.dictionary_auto_accept = enabled;
    write_settings(&app, settings);
    settings_changed(&app, "dictionary_auto_accept", enabled);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_dictionary_learn_from_edits_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.dictionary_learn_from_edits = enabled;
    write_settings(&app, settings);
    settings_changed(&app, "dictionary_learn_from_edits", enabled);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_dictionary_suggest_from_llm_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.dictionary_suggest_from_llm = enabled;
    write_settings(&app, settings);
    settings_changed(&app, "dictionary_suggest_from_llm", enabled);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_dictionary_suggest_from_meetings_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.dictionary_suggest_from_meetings = enabled;
    write_settings(&app, settings);
    settings_changed(&app, "dictionary_suggest_from_meetings", enabled);
    Ok(())
}
