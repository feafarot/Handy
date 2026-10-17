//! Meetings that start, and stop, on their own.
//!
//! Every few seconds the apps holding the microphone are looked up (see the
//! platform module) and sorted out (`apps`); `detector` decides from that
//! when a meeting starts, whether to ask first, and when it is over.
//! Questions and notices appear in a small window of their own
//! (`prompt_window`), which answers through [`MeetingAutoStarter::answer`].
//!
//! Platform support: Windows. Another platform plugs in by providing
//! `apps_holding_microphone` and `own_id` and flipping `SUPPORTED`.

mod apps;
mod detector;
mod prompt_window;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
use windows as platform;

/// Placeholder for platforms that cannot tell which apps use the microphone.
#[cfg(not(target_os = "windows"))]
mod platform {
    use super::apps::MicApp;

    pub fn apps_holding_microphone() -> Vec<MicApp> {
        Vec::new()
    }

    pub fn own_id() -> String {
        String::new()
    }
}

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use log::{info, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Emitter, Manager};
use tauri_specta::Event;

pub use apps::AppKind;
use apps::Candidate;
use detector::{Action, Answer, Detector, MeetingPhase, Policy, PromptKind};

use super::{MeetingManager, MeetingState};
use crate::settings::{get_settings, write_settings, MeetingAutoStart, MeetingBlockedApp};

/// Whether this platform can detect meetings.
pub const SUPPORTED: bool = cfg!(target_os = "windows");

const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// A question or notice about a detected meeting, for the prompt window.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct MeetingPrompt {
    /// Answers name the prompt they answer.
    pub id: u32,
    pub request: MeetingPromptRequest,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MeetingPromptRequest {
    /// Whether to record a possible meeting.
    Start {
        /// The app holding the microphone, e.g. "Google Chrome".
        app: String,
        app_kind: AppKind,
        /// The web meeting a browser shows, e.g. "Google Meet".
        service: Option<String>,
        /// The window title that gave it away.
        hint: Option<String>,
        /// Whether "never for this app" is offered.
        can_block: bool,
    },
    /// A recording started without asking; "never for this app" stops it.
    Started { app: String },
    /// Whether to stop a meeting whose app let go of the microphone.
    Stop { app: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingPromptEvent {
    pub prompt: Option<MeetingPrompt>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum MeetingPromptAnswer {
    Record,
    Stop,
    /// Never for this app (full-auto only).
    Block,
    Dismiss,
}

pub struct MeetingAutoStarter {
    app: AppHandle,
    detector: Mutex<Detector>,
    /// The prompt on screen.
    shown: Mutex<Option<u32>>,
    /// The meeting being recorded whose app is known (or was given and then
    /// cleared by the user, which is left alone).
    app_known: Mutex<Option<i64>>,
    own_id: String,
}

impl MeetingAutoStarter {
    pub fn new(app: &AppHandle) -> Self {
        Self {
            app: app.clone(),
            detector: Mutex::new(Detector::default()),
            shown: Mutex::new(None),
            app_known: Mutex::new(None),
            own_id: platform::own_id(),
        }
    }

    /// Starts watching the microphone in the background, where supported.
    pub fn spawn(self: &Arc<Self>) {
        if !SUPPORTED {
            return;
        }
        let this = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("meeting-auto-start".into())
            .spawn(move || loop {
                std::thread::sleep(POLL_INTERVAL);
                this.tick();
            });
        if let Err(e) = spawned {
            warn!("Failed to start meeting detection: {e}");
        }
    }

    fn lock(&self) -> MutexGuard<'_, Detector> {
        self.detector.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn tick(&self) {
        let settings = get_settings(&self.app);
        let policy = Policy {
            mode: settings.meeting_auto_start,
            auto_stop: settings.meeting_auto_stop,
            blocklist: settings
                .meeting_auto_start_blocklist
                .into_iter()
                .map(|app| app.id)
                .collect(),
        };
        let state = self.app.state::<Arc<MeetingManager>>().state();
        let phase = match &state {
            MeetingState::Idle => MeetingPhase::Idle,
            MeetingState::Recording { meeting } => MeetingPhase::Recording(meeting.id),
            MeetingState::Finishing { .. } => MeetingPhase::Finishing,
        };
        let unknown_app = self.unknown_app(&state);
        let watching = policy.mode != MeetingAutoStart::Manual
            || unknown_app.is_some()
            || self.lock().is_watching();
        let holding = if watching { self.holding() } else { Vec::new() };
        if let (Some(meeting_id), Some(best)) = (unknown_app, Detector::best(&holding)) {
            self.name_app(meeting_id, &best.label());
        }
        let actions = self.lock().tick(Instant::now(), holding, phase, &policy);
        self.perform(actions);
    }

    /// The meeting being recorded, if nobody knows its app yet: one started
    /// by hand before the call.
    fn unknown_app(&self, state: &MeetingState) -> Option<i64> {
        let MeetingState::Recording { meeting } = state else {
            return None;
        };
        let mut known = self
            .app_known
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *known == Some(meeting.id) {
            return None;
        }
        if meeting.app.is_some() {
            *known = Some(meeting.id);
            return None;
        }
        Some(meeting.id)
    }

    fn name_app(&self, meeting_id: i64, app: &str) {
        let meetings = self.app.state::<Arc<MeetingManager>>();
        match meetings.set_app(meeting_id, Some(app)) {
            Ok(()) => {
                info!("Meeting {meeting_id} is held in {app}");
                *self
                    .app_known
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(meeting_id);
            }
            Err(e) => warn!("Could not note the app of meeting {meeting_id}: {e}"),
        }
    }

    fn holding(&self) -> Vec<Candidate> {
        platform::apps_holding_microphone()
            .into_iter()
            .filter_map(|app| apps::classify(app, &self.own_id))
            .collect()
    }

    /// What a meeting started now is most likely held in, e.g. "Zoom".
    pub fn current_app(&self) -> Option<String> {
        Detector::best(&self.holding()).map(Candidate::label)
    }

    pub fn prompt(&self) -> Option<MeetingPrompt> {
        let detector = self.lock();
        let prompt = detector.prompt()?;
        let request = match &prompt.kind {
            PromptKind::Start {
                candidate,
                can_block,
            } => MeetingPromptRequest::Start {
                app: candidate.name.clone(),
                app_kind: candidate.kind,
                service: candidate.service.map(str::to_string),
                hint: candidate.hint.clone(),
                can_block: *can_block,
            },
            PromptKind::Started { candidate, .. } => MeetingPromptRequest::Started {
                app: candidate.label(),
            },
            PromptKind::Stop { app, .. } => MeetingPromptRequest::Stop { app: app.clone() },
        };
        Some(MeetingPrompt {
            id: prompt.id,
            request,
        })
    }

    /// The user's answer to prompt `id`.
    pub fn answer(&self, id: u32, answer: MeetingPromptAnswer) {
        let answer = match answer {
            MeetingPromptAnswer::Record => Answer::Record,
            MeetingPromptAnswer::Stop => Answer::Stop,
            MeetingPromptAnswer::Block => Answer::Block,
            MeetingPromptAnswer::Dismiss => Answer::Dismiss,
        };
        let actions = self.lock().answer(id, answer);
        self.perform(actions);
    }

    fn perform(&self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Start {
                    candidate,
                    automatic,
                } => self.start(candidate, automatic),
                Action::Stop { meeting_id } => self.stop(meeting_id),
                Action::Block { id, name } => self.block(id, name),
            }
        }
        self.sync_prompt();
    }

    fn start(&self, candidate: Candidate, automatic: bool) {
        let meetings = self.app.state::<Arc<MeetingManager>>();
        let system_audio = get_settings(&self.app).meeting_system_audio;
        // Untitled: the UI names it by its date, in the user's language.
        match meetings.start("", system_audio, Some(&candidate.label())) {
            Ok(meeting) => {
                info!(
                    "Recording meeting {} detected in {} ({})",
                    meeting.id,
                    candidate.label(),
                    if automatic {
                        "automatically"
                    } else {
                        "as asked"
                    }
                );
                self.lock()
                    .started(meeting.id, &candidate, automatic, Instant::now());
            }
            Err(e) => warn!(
                "Could not record the meeting detected in {}: {e}",
                candidate.label()
            ),
        }
    }

    fn stop(&self, meeting_id: i64) {
        let meetings = self.app.state::<Arc<MeetingManager>>();
        // It may have been stopped, and another started, in the meantime.
        if !matches!(meetings.state(), MeetingState::Recording { meeting } if meeting.id == meeting_id)
        {
            return;
        }
        info!("Meeting {meeting_id} is over: its app let go of the microphone");
        if let Err(e) = meetings.stop() {
            warn!("Could not stop meeting {meeting_id}: {e}");
        }
    }

    fn block(&self, id: String, name: String) {
        let mut settings = get_settings(&self.app);
        if settings
            .meeting_auto_start_blocklist
            .iter()
            .any(|app| app.id == id)
        {
            return;
        }
        info!("Meeting detection now leaves {name} ({id}) alone");
        settings
            .meeting_auto_start_blocklist
            .push(MeetingBlockedApp { id, name });
        write_settings(&self.app, settings);
        let _ = self.app.emit(
            "settings-changed",
            serde_json::json!({ "setting": "meeting_auto_start_blocklist" }),
        );
    }

    /// Shows the current prompt, or hides the window when there is none.
    fn sync_prompt(&self) {
        let prompt = self.prompt();
        let id = prompt.as_ref().map(|prompt| prompt.id);
        {
            let mut shown = self.shown.lock().unwrap_or_else(PoisonError::into_inner);
            if *shown == id {
                return;
            }
            *shown = id;
        }
        let visible = prompt.is_some();
        if let Err(e) = (MeetingPromptEvent { prompt }).emit(&self.app) {
            warn!("Failed to emit the meeting prompt: {e}");
        }
        if visible {
            prompt_window::show(&self.app);
        } else {
            prompt_window::hide(&self.app);
        }
    }
}
