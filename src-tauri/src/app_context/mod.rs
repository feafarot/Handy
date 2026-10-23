//! App context: what the user is dictating into.
//!
//! When enabled, dictating with post-processing captures the foreground app,
//! the text around the cursor and, as a fallback, OCR of the window (see
//! `capture`), and hands it to the post-processing LLM so it can spell names
//! and identifiers the way they appear on screen and match the app's
//! conventions.
//!
//! Capture starts with the recording and runs on its own thread while the
//! user speaks, so it only adds latency if it is still running when
//! transcription finishes. Optionally the captured context is sent to the LLM
//! right away as a one-token request, so a local server (llama.cpp, Ollama)
//! already holds the prompt prefix in its KV cache, and has the model loaded,
//! when the real request arrives.
//!
//! Platform support: Windows. Another platform plugs in by providing the
//! `platform` functions `foreground_app`, `focused_field` and `screen_text`
//! (see `windows.rs`) and flipping `SUPPORTED`.

// The shared capture logic is compiled everywhere (for the unit tests), but
// only Windows has a platform implementation that consumes all of it.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

mod capture;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
use windows as platform;

/// Placeholder for platforms without app context support: captures nothing.
#[cfg(not(target_os = "windows"))]
mod platform {
    use super::capture::{AppInfo, FocusedField};

    pub struct Window;

    pub fn foreground_app() -> Option<(AppInfo, Window)> {
        None
    }

    pub fn focused_field(_window: &Window) -> FocusedField {
        FocusedField::Unavailable
    }

    pub fn screen_text(_window: &Window, _cursor_y: Option<f64>, _max: usize) -> Option<String> {
        None
    }
}

/// Text cleanup other accessibility readers share.
pub(crate) use capture::clean_text;
pub use capture::{AppContext, CaptureOptions};
/// Process and window lookups other features share.
#[cfg(target_os = "windows")]
pub(crate) use windows::{ensure_mta, file_description, process_image_path, window_title};

use log::{debug, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;
use tauri::async_runtime::JoinHandle;

use crate::settings::AppSettings;

/// Whether this platform can capture app context.
pub const SUPPORTED: bool = cfg!(target_os = "windows");

/// How long post-processing waits for a capture that is still running when
/// transcription finishes. Past this it uses whatever steps are done.
const CAPTURE_GRACE: Duration = Duration::from_millis(1000);

/// How long post-processing waits for the cache-priming request. The real
/// request should land on the primed cache rather than race the priming
/// request into another server slot, but a stuck priming request must not
/// hold up the dictation.
const PRELOAD_GRACE: Duration = Duration::from_secs(4);

/// Starts the cache-priming request for a captured context.
pub type Preload = Box<dyn FnOnce(&AppContext) -> Option<JoinHandle<()>> + Send>;

impl CaptureOptions {
    pub fn from_settings(settings: &AppSettings) -> Self {
        Self {
            focused_text: settings.app_context_focused_text,
            screen_text: settings.app_context_screen_text,
        }
    }
}

#[derive(Default)]
struct Progress {
    context: Option<AppContext>,
    preload: Option<JoinHandle<()>>,
    done: bool,
}

#[derive(Default)]
struct Shared {
    progress: Mutex<Progress>,
    changed: Condvar,
    abandoned: AtomicBool,
}

impl Shared {
    fn update(&self, change: impl FnOnce(&mut Progress)) {
        let mut progress = self.progress.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut progress);
        self.changed.notify_all();
    }
}

/// One capture, started when recording starts and consumed by
/// post-processing.
pub struct CaptureSession {
    shared: Arc<Shared>,
}

impl CaptureSession {
    /// Starts capturing on a worker thread. `preload` runs on that thread once
    /// the capture is complete.
    pub fn start(options: CaptureOptions, preload: Option<Preload>) -> Self {
        let shared = Arc::new(Shared::default());
        let worker = Arc::clone(&shared);

        let spawned = std::thread::Builder::new()
            .name("app-context".into())
            .spawn(move || {
                let context = capture::capture(options, |partial| {
                    worker.update(|progress| progress.context = Some(partial.clone()));
                });
                let preload = match (&context, preload) {
                    (Some(context), Some(preload)) if !worker.abandoned.load(Ordering::Acquire) => {
                        preload(context)
                    }
                    _ => None,
                };
                worker.update(|progress| {
                    progress.context = context;
                    progress.preload = preload;
                    progress.done = true;
                });
            });

        if let Err(e) = spawned {
            warn!("Failed to start app context capture: {e}");
            shared.update(|progress| progress.done = true);
        }

        Self { shared }
    }

    /// Waits, within bounds, for the capture and any cache-priming request,
    /// then returns whatever context was captured.
    pub async fn finish(self) -> Option<AppContext> {
        let shared = Arc::clone(&self.shared);
        let (context, preload) = tauri::async_runtime::spawn_blocking(move || {
            let progress = shared
                .progress
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let (mut progress, wait) = shared
                .changed
                .wait_timeout_while(progress, CAPTURE_GRACE, |progress| !progress.done)
                .unwrap_or_else(PoisonError::into_inner);
            if wait.timed_out() {
                debug!(
                    "App context capture still running after transcription; using what is ready"
                );
            }
            (progress.context.clone(), progress.preload.take())
        })
        .await
        .unwrap_or_default();

        if let Some(preload) = preload {
            if tokio::time::timeout(PRELOAD_GRACE, preload).await.is_err() {
                debug!("Prompt cache priming still running; sending the request anyway");
            }
        }

        context
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        // A dropped session (cancelled recording, no post-processing) has no
        // use for a primed cache.
        self.shared.abandoned.store(true, Ordering::Release);
    }
}
