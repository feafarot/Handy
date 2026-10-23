//! Reads the focused text field through UI Automation, for the edit monitor.
//!
//! The element with keyboard focus is captured once, right after the paste,
//! and re-read on every poll, so the watch follows the field even after the
//! user moves on to another window. All calls happen on the monitor's own
//! thread inside the process-wide MTA (see `app_context::windows`).

use log::debug;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation2, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationTextPattern2, IUIAutomationTextRange, IUIAutomationValuePattern,
    TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, TextUnit_Character,
    UIA_TextPattern2Id, UIA_TextPatternId, UIA_ValuePatternId,
};

use super::edit_monitor::{FieldReader, FocusedField};
use crate::app_context::{clean_text, ensure_mta};

/// Upper bound for a single UI Automation call into the target app.
const UIA_TIMEOUT_MS: u32 = 1000;
/// The most text read from a field in one go. Enough for any chat box or
/// note; a long document is read around the caret instead.
const MAX_DOCUMENT_CHARS: i32 = 30_000;
/// Text read on either side of the caret when the document is too long.
const CARET_WINDOW_CHARS: i32 = 6_000;

pub struct UiaFieldReader;

impl FieldReader for UiaFieldReader {
    fn capture(&self) -> Option<Box<dyn FocusedField>> {
        if !ensure_mta() {
            return None;
        }
        match capture_focused() {
            Ok(field) => field.map(|f| Box::new(f) as Box<dyn FocusedField>),
            Err(e) => {
                debug!("Dictionary: UI Automation could not read the focused field: {e}");
                None
            }
        }
    }
}

struct UiaField {
    element: IUIAutomationElement,
    origin: String,
}

fn capture_focused() -> windows::core::Result<Option<UiaField>> {
    let automation: IUIAutomation2 =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)? };
    unsafe {
        automation.SetConnectionTimeout(UIA_TIMEOUT_MS)?;
        automation.SetTransactionTimeout(UIA_TIMEOUT_MS)?;
    }
    let element = unsafe { automation.GetFocusedElement()? };
    if unsafe { element.CurrentIsPassword() }.is_ok_and(|protected| protected.as_bool()) {
        debug!("Dictionary: the focused field is protected; not watching it");
        return Ok(None);
    }
    let has_text = unsafe { element.GetCurrentPattern(UIA_TextPatternId) }.is_ok()
        || unsafe { element.GetCurrentPattern(UIA_ValuePatternId) }.is_ok();
    if !has_text {
        return Ok(None);
    }

    let mut pid = 0u32;
    if let Ok(id) = unsafe { element.CurrentProcessId() } {
        pid = id as u32;
    }
    let origin = crate::app_context::process_image_path(pid)
        .as_deref()
        .and_then(crate::app_context::file_description)
        .or_else(|| {
            crate::app_context::process_image_path(pid).and_then(|path| {
                std::path::Path::new(&path)
                    .file_stem()
                    .map(|name| name.to_string_lossy().into_owned())
            })
        })
        .unwrap_or_else(|| "an app".to_string());

    Ok(Some(UiaField { element, origin }))
}

impl FocusedField for UiaField {
    fn text(&self) -> Option<String> {
        match read_text(&self.element) {
            Ok(text) => Some(text),
            Err(e) => {
                debug!("Dictionary: reading the watched field failed: {e}");
                None
            }
        }
    }

    fn origin(&self) -> String {
        self.origin.clone()
    }
}

fn read_text(element: &IUIAutomationElement) -> windows::core::Result<String> {
    if let Ok(pattern) =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
    {
        let document = unsafe { pattern.DocumentRange()? };
        let text = unsafe { document.GetText(MAX_DOCUMENT_CHARS)? }.to_string();
        // A document longer than the cap: read around the caret, where the
        // dictation went and where the user is editing.
        if text.chars().count() as i32 >= MAX_DOCUMENT_CHARS {
            if let Some(window) = caret_window(element, &pattern, &document) {
                return Ok(clean_text(&window));
            }
        }
        return Ok(clean_text(&text));
    }
    let pattern =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)? };
    Ok(clean_text(&unsafe { pattern.CurrentValue()? }.to_string()))
}

fn caret_window(
    element: &IUIAutomationElement,
    pattern: &IUIAutomationTextPattern,
    document: &IUIAutomationTextRange,
) -> Option<String> {
    let caret = unsafe { pattern.GetSelection() }
        .ok()
        .and_then(|ranges| {
            (unsafe { ranges.Length() }.unwrap_or(0) > 0)
                .then(|| unsafe { ranges.GetElement(0) }.ok())
                .flatten()
        })
        .or_else(|| {
            let pattern = unsafe {
                element.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id)
            }
            .ok()?;
            let mut active = windows::core::BOOL::default();
            unsafe { pattern.GetCaretRange(&mut active) }.ok()
        })?;
    let window = unsafe { caret.Clone() }.ok()?;
    unsafe {
        window
            .MoveEndpointByUnit(
                TextPatternRangeEndpoint_Start,
                TextUnit_Character,
                -CARET_WINDOW_CHARS,
            )
            .ok()?;
        window
            .MoveEndpointByUnit(
                TextPatternRangeEndpoint_End,
                TextUnit_Character,
                CARET_WINDOW_CHARS,
            )
            .ok()?;
        if window
            .CompareEndpoints(
                TextPatternRangeEndpoint_Start,
                document,
                TextPatternRangeEndpoint_Start,
            )
            .ok()?
            < 0
        {
            window
                .MoveEndpointByRange(
                    TextPatternRangeEndpoint_Start,
                    document,
                    TextPatternRangeEndpoint_Start,
                )
                .ok()?;
        }
        if window
            .CompareEndpoints(
                TextPatternRangeEndpoint_End,
                document,
                TextPatternRangeEndpoint_End,
            )
            .ok()?
            > 0
        {
            window
                .MoveEndpointByRange(
                    TextPatternRangeEndpoint_End,
                    document,
                    TextPatternRangeEndpoint_End,
                )
                .ok()?;
        }
        Some(window.GetText(-1).ok()?.to_string())
    }
}
