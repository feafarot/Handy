use crate::meeting::auto::{MeetingAutoStarter, MeetingPrompt, MeetingPromptAnswer};
use crate::meeting::{Meeting, MeetingDetail, MeetingManager, MeetingState};
use crate::settings::{self, MeetingAutoStart, MeetingAutoStop, MeetingBlockedApp};
use std::sync::Arc;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
#[specta::specta]
pub fn start_meeting(
    app: AppHandle,
    meetings: State<'_, Arc<MeetingManager>>,
    auto_starter: State<'_, Arc<MeetingAutoStarter>>,
    title: String,
) -> Result<Meeting, String> {
    let system_audio = settings::get_settings(&app).meeting_system_audio;
    let held_in = auto_starter.current_app();
    meetings
        .start(&title, system_audio, held_in.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn stop_meeting(meetings: State<'_, Arc<MeetingManager>>) -> Result<(), String> {
    meetings.stop().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn get_meeting_state(meetings: State<'_, Arc<MeetingManager>>) -> MeetingState {
    meetings.state()
}

#[tauri::command]
#[specta::specta]
pub fn list_meetings(meetings: State<'_, Arc<MeetingManager>>) -> Result<Vec<Meeting>, String> {
    meetings.list().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn get_meeting(
    meetings: State<'_, Arc<MeetingManager>>,
    id: i64,
) -> Result<Option<MeetingDetail>, String> {
    meetings.get(id).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn rename_meeting(
    meetings: State<'_, Arc<MeetingManager>>,
    id: i64,
    title: String,
) -> Result<(), String> {
    meetings.rename(id, &title).map_err(|e| e.to_string())
}

/// Sets the app a meeting was held in; an empty or missing name means
/// unknown.
#[tauri::command]
#[specta::specta]
pub fn set_meeting_app(
    meetings: State<'_, Arc<MeetingManager>>,
    id: i64,
    app: Option<String>,
) -> Result<(), String> {
    meetings
        .set_app(id, app.as_deref())
        .map_err(|e| e.to_string())
}

/// Starts telling the other participants apart; see `MeetingSpeakersEvent`.
#[tauri::command]
#[specta::specta]
pub fn identify_meeting_speakers(
    meetings: State<'_, Arc<MeetingManager>>,
    id: i64,
) -> Result<(), String> {
    meetings.identify_speakers(id).map_err(|e| e.to_string())
}

/// Starts writing a meeting's notes; see `MeetingNotesEvent`.
#[tauri::command]
#[specta::specta]
pub fn write_meeting_notes(
    meetings: State<'_, Arc<MeetingManager>>,
    id: i64,
) -> Result<(), String> {
    meetings.write_notes(id).map_err(|e| e.to_string())
}

/// Names one of a meeting's speakers; an empty or missing name goes back to
/// the number.
#[tauri::command]
#[specta::specta]
pub fn rename_meeting_speaker(
    meetings: State<'_, Arc<MeetingManager>>,
    id: i64,
    speaker: u32,
    name: Option<String>,
) -> Result<(), String> {
    meetings
        .rename_speaker(id, speaker, name.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn delete_meeting(meetings: State<'_, Arc<MeetingManager>>, id: i64) -> Result<(), String> {
    meetings.delete(id).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn open_meeting_folder(
    app: AppHandle,
    meetings: State<'_, Arc<MeetingManager>>,
    id: i64,
) -> Result<(), String> {
    let dir = meetings.audio_dir(id);
    app.opener()
        .open_path(dir.to_string_lossy(), None::<String>)
        .map_err(|e| e.to_string())
}

/// Whether this platform can record the other participants.
#[tauri::command]
#[specta::specta]
pub fn is_meeting_system_audio_supported() -> bool {
    crate::meeting::SYSTEM_AUDIO_SUPPORTED
}

#[tauri::command]
#[specta::specta]
pub fn change_meeting_system_audio_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.meeting_system_audio = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_meeting_auto_notes_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.meeting_auto_notes = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Whether this platform can tell when a meeting starts.
#[tauri::command]
#[specta::specta]
pub fn is_meeting_auto_start_supported() -> bool {
    crate::meeting::auto::SUPPORTED
}

#[tauri::command]
#[specta::specta]
pub fn change_meeting_auto_start_setting(
    app: AppHandle,
    mode: MeetingAutoStart,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.meeting_auto_start = mode;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_meeting_auto_stop_setting(
    app: AppHandle,
    mode: MeetingAutoStop,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.meeting_auto_stop = mode;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_meeting_auto_start_blocklist_setting(
    app: AppHandle,
    apps: Vec<MeetingBlockedApp>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.meeting_auto_start_blocklist = apps;
    settings::write_settings(&app, settings);
    Ok(())
}

/// The question or notice about a detected meeting on screen, if any; see
/// `MeetingPromptEvent`.
#[tauri::command]
#[specta::specta]
pub fn get_meeting_prompt(
    auto_starter: State<'_, Arc<MeetingAutoStarter>>,
) -> Option<MeetingPrompt> {
    auto_starter.prompt()
}

#[tauri::command]
#[specta::specta]
pub fn answer_meeting_prompt(
    auto_starter: State<'_, Arc<MeetingAutoStarter>>,
    id: u32,
    answer: MeetingPromptAnswer,
) {
    // Starting a recording opens devices: keep it off the main thread.
    let auto_starter = Arc::clone(&auto_starter);
    std::thread::spawn(move || auto_starter.answer(id, answer));
}
