//! The small window asking whether to record, or stop, a detected meeting,
//! in the corner of the primary screen. Made on first use, then hidden and
//! shown again. It never takes focus, so popping up mid-call does not steal
//! the keyboard from the meeting app.

#[cfg(target_os = "windows")]
pub use imp::{hide, show};

#[cfg(target_os = "windows")]
mod imp {
    use log::warn;
    use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindow};

    const LABEL: &str = "meeting_prompt";
    /// Logical size; the card in `src/meeting-prompt` fits inside it.
    const WIDTH: f64 = 380.0;
    const HEIGHT: f64 = 150.0;
    /// Gap to the edges of the screen's work area.
    const MARGIN: f64 = 12.0;

    pub fn show(app: &AppHandle) {
        let window = match app.get_webview_window(LABEL) {
            Some(window) => window,
            None => match build(app) {
                Ok(window) => window,
                Err(e) => {
                    warn!("Failed to create the meeting prompt window: {e}");
                    return;
                }
            },
        };
        place(app, &window);
        if let Err(e) = window.show() {
            warn!("Failed to show the meeting prompt: {e}");
        }
    }

    pub fn hide(app: &AppHandle) {
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.hide();
        }
    }

    fn build(app: &AppHandle) -> tauri::Result<WebviewWindow> {
        let mut builder = tauri::WebviewWindowBuilder::new(
            app,
            LABEL,
            WebviewUrl::App("src/meeting-prompt/index.html".into()),
        )
        .title("Handy")
        .resizable(false)
        .inner_size(WIDTH, HEIGHT)
        .shadow(false)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        .accept_first_mouse(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .transparent(true)
        .focusable(false)
        .focused(false)
        .visible(false);
        if let Some(data_dir) = crate::portable::data_dir() {
            builder = builder.data_directory(data_dir.join("webview"));
        }
        builder.build()
    }

    /// Bottom right of the primary screen, above the taskbar.
    fn place(app: &AppHandle, window: &WebviewWindow) {
        let Some(monitor) = app.primary_monitor().ok().flatten() else {
            return;
        };
        let area = monitor.work_area();
        let scale = monitor.scale_factor();
        let x = f64::from(area.position.x) + f64::from(area.size.width) - (WIDTH + MARGIN) * scale;
        let y =
            f64::from(area.position.y) + f64::from(area.size.height) - (HEIGHT + MARGIN) * scale;
        if let Err(e) = window.set_position(PhysicalPosition::new(x as i32, y as i32)) {
            warn!("Failed to place the meeting prompt: {e}");
        }
    }
}

/// Prompts only come up where meetings are detected.
#[cfg(not(target_os = "windows"))]
pub fn show(_app: &tauri::AppHandle) {}

#[cfg(not(target_os = "windows"))]
pub fn hide(_app: &tauri::AppHandle) {}
