use crate::meeting::{Meeting, MeetingDetail, MeetingManager, MeetingState};
use crate::settings;
use std::sync::Arc;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
#[specta::specta]
pub fn start_meeting(
    app: AppHandle,
    meetings: State<'_, Arc<MeetingManager>>,
    title: String,
) -> Result<Meeting, String> {
    let system_audio = settings::get_settings(&app).meeting_system_audio;
    meetings
        .start(&title, system_audio)
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
