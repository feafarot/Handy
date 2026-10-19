use crate::meeting::auto::{MeetingAutoStarter, MeetingPrompt, MeetingPromptAnswer};
use crate::meeting::{Meeting, MeetingDetail, MeetingManager, MeetingState};
use crate::settings::{
    self, LLMPrompt, MeetingAutoStart, MeetingAutoStop, MeetingBlockedApp, MeetingNotesWriter,
};
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

#[tauri::command]
#[specta::specta]
pub fn change_meeting_guess_speaker_names_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.meeting_guess_speaker_names = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

/// Sets what writes a meeting's notes; `None` follows the setting.
#[tauri::command]
#[specta::specta]
pub fn set_meeting_notes_writer(
    meetings: State<'_, Arc<MeetingManager>>,
    id: i64,
    writer: Option<MeetingNotesWriter>,
) -> Result<(), String> {
    meetings
        .set_notes_writer(id, writer)
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn change_meeting_notes_writer_setting(
    app: AppHandle,
    writer: MeetingNotesWriter,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.meeting_notes_writer = writer;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_meeting_notes_claude_model_setting(
    app: AppHandle,
    model: String,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.meeting_notes_claude_model = model.trim().to_string();
    settings::write_settings(&app, settings);
    Ok(())
}

/// The installed Claude Code's version, or why it can't be used.
#[tauri::command]
#[specta::specta]
pub async fn get_claude_code_version() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(crate::meeting::claude_code_version)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn change_meeting_notes_context_tokens_setting(
    app: AppHandle,
    tokens: u32,
) -> Result<(), String> {
    if tokens < 2048 {
        return Err("The context must be at least 2048 tokens".to_string());
    }
    let mut settings = settings::get_settings(&app);
    settings.meeting_notes_context_tokens = tokens;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn add_meeting_notes_prompt(
    app: AppHandle,
    name: String,
    prompt: String,
) -> Result<LLMPrompt, String> {
    let mut settings = settings::get_settings(&app);
    let new_prompt = LLMPrompt {
        id: format!("meeting_prompt_{}", chrono::Utc::now().timestamp_millis()),
        name,
        prompt,
    };
    settings.meeting_notes_prompts.push(new_prompt.clone());
    settings::write_settings(&app, settings);
    Ok(new_prompt)
}

#[tauri::command]
#[specta::specta]
pub fn update_meeting_notes_prompt(
    app: AppHandle,
    id: String,
    name: String,
    prompt: String,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let existing = settings
        .meeting_notes_prompts
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Prompt with id '{id}' not found"))?;
    existing.name = name;
    existing.prompt = prompt;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn delete_meeting_notes_prompt(app: AppHandle, id: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    if settings.meeting_notes_prompts.len() <= 1 {
        return Err("Cannot delete the last prompt".to_string());
    }
    let original_len = settings.meeting_notes_prompts.len();
    settings.meeting_notes_prompts.retain(|p| p.id != id);
    if settings.meeting_notes_prompts.len() == original_len {
        return Err(format!("Prompt with id '{id}' not found"));
    }
    if settings.meeting_notes_selected_prompt_id.as_ref() == Some(&id) {
        settings.meeting_notes_selected_prompt_id =
            settings.meeting_notes_prompts.first().map(|p| p.id.clone());
    }
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn set_meeting_notes_selected_prompt(app: AppHandle, id: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    if !settings.meeting_notes_prompts.iter().any(|p| p.id == id) {
        return Err(format!("Prompt with id '{id}' not found"));
    }
    settings.meeting_notes_selected_prompt_id = Some(id);
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
