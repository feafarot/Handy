//! Which apps hold the microphone on Windows, from the privacy records behind
//! the taskbar's microphone indicator: `ConsentStore\microphone` keeps, per
//! app, when it last started and last stopped using a microphone, so an app
//! using one right now started after it last stopped. Packaged apps are keyed
//! by package family name, the rest (under `NonPackaged`) by executable path
//! with `#` for `\`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
};
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

use super::apps::MicApp;
use crate::app_context::{file_description, process_image_path, window_title};

const MICROPHONE: &str =
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

pub fn apps_holding_microphone() -> Vec<MicApp> {
    let Ok(root) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(MICROPHONE) else {
        return Vec::new();
    };
    let mut apps = Vec::new();
    let mut paths = Vec::new();
    for name in root.enum_keys().flatten() {
        if name == "NonPackaged" {
            let Ok(non_packaged) = root.open_subkey(&name) else {
                continue;
            };
            for name in non_packaged.enum_keys().flatten() {
                if non_packaged
                    .open_subkey(&name)
                    .is_ok_and(|key| in_use(&key))
                {
                    paths.push(name.replace('#', "\\"));
                }
            }
        } else if root.open_subkey(&name).is_ok_and(|key| in_use(&key)) {
            apps.push(MicApp {
                id: name.to_lowercase(),
                ..MicApp::default()
            });
        }
    }
    if paths.is_empty() {
        return apps;
    }
    let mut titles = window_titles();
    for path in paths {
        let Some(id) = file_name(&path) else {
            continue;
        };
        apps.push(MicApp {
            name: description(&path),
            window_titles: titles.remove(&id).unwrap_or_default(),
            id,
        });
    }
    apps
}

/// Handy's own [`MicApp::id`].
pub fn own_id() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| file_name(&path.to_string_lossy()))
        .unwrap_or_default()
}

fn in_use(key: &RegKey) -> bool {
    let start: u64 = key.get_value("LastUsedTimeStart").unwrap_or(0);
    let stop: u64 = key.get_value("LastUsedTimeStop").unwrap_or(0);
    start > stop
}

fn file_name(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
}

/// The executable's own name, cached: polled every few seconds.
fn description(path: &str) -> Option<String> {
    static NAMES: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(path.to_string())
        .or_insert_with(|| file_description(path))
        .clone()
}

/// Titles of the visible top-level windows, by executable file name.
fn window_titles() -> HashMap<String, Vec<String>> {
    unsafe extern "system" fn collect(hwnd: HWND, windows: LPARAM) -> BOOL {
        // SAFETY: `windows` is the Vec passed to EnumWindows below, alive
        // for the whole enumeration.
        let windows = unsafe { &mut *(windows.0 as *mut Vec<HWND>) };
        if unsafe { IsWindowVisible(hwnd) }.as_bool() {
            windows.push(hwnd);
        }
        true.into()
    }

    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: the callback only touches `windows`, which outlives the call.
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut windows as *mut _ as isize)) };

    let mut images: HashMap<u32, Option<String>> = HashMap::new();
    let mut titles: HashMap<String, Vec<String>> = HashMap::new();
    for hwnd in windows {
        let Some(title) = window_title(hwnd) else {
            continue;
        };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let image = images
            .entry(pid)
            .or_insert_with(|| process_image_path(pid).and_then(|path| file_name(&path)));
        if let Some(image) = image {
            titles.entry(image.clone()).or_default().push(title);
        }
    }
    titles
}
