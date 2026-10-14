//! Windows app context capture.
//!
//! - Foreground app: `GetForegroundWindow`, the owning process's image path,
//!   and the image's version-info `FileDescription` as a readable name.
//! - Focused field: UI Automation. Its connection and transaction timeouts
//!   are lowered from the 20 s default, so a hung target app costs at most
//!   about a second (and that second is spent while the user is speaking).
//! - Screen text: a GDI copy of the window's on-screen pixels recognized by
//!   the built-in `Windows.Media.Ocr` engine, which uses the OCR languages
//!   installed for the user profile. No extra runtime dependencies.
//!
//! All calls run on the capture worker thread. COM is used through the
//! process-wide multithreaded apartment, kept alive with
//! `CoIncrementMTAUsage`, so the worker needs no per-thread COM setup.

use log::debug;
use std::ffi::c_void;
use std::path::Path;
use std::sync::OnceLock;

use windows::core::{w, BOOL, HSTRING, PWSTR};
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::Foundation::{CloseHandle, HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, SRCCOPY,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoIncrementMTAUsage, CLSCTX_INPROC_SERVER, SAFEARRAY,
};
use windows::Win32::System::Ole::SafeArrayDestroy;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation2, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationTextPattern2, IUIAutomationTextRange, IUIAutomationValuePattern,
    TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, TextUnit_Character,
    UIA_TextPattern2Id, UIA_TextPatternId, UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetGUIThreadInfo, GetSystemMetrics, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, GUITHREADINFO, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use super::capture::{
    clean_text, head_chars, repair_ocr_text, select_screen_lines, tail_chars, AppInfo,
    FocusedField, FocusedText, ScreenLine, MAX_AFTER_CURSOR_CHARS, MAX_BEFORE_CURSOR_CHARS,
    MAX_SELECTION_CHARS,
};

/// Upper bound for a single UI Automation call into the target app.
const UIA_TIMEOUT_MS: u32 = 1000;

/// The foreground window captured at the start of a dictation.
pub struct Window {
    hwnd: HWND,
    thread_id: u32,
}

/// Keeps the process-wide MTA alive so the worker thread can use COM (UI
/// Automation) and WinRT (OCR) without initializing COM itself, and so the
/// cached OCR engine stays valid between captures.
fn ensure_mta() -> bool {
    static MTA: OnceLock<bool> = OnceLock::new();
    *MTA.get_or_init(|| match unsafe { CoIncrementMTAUsage() } {
        Ok(_) => true,
        Err(e) => {
            debug!("App context: CoIncrementMTAUsage failed: {e}");
            false
        }
    })
}

pub fn foreground_app() -> Option<(AppInfo, Window)> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return None;
    }

    let mut pid = 0u32;
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let image = process_image_path(pid);
    let process = image
        .as_deref()
        .and_then(|path| Path::new(path).file_name())
        .map(|name| name.to_string_lossy().into_owned());
    let name = image.as_deref().and_then(file_description);

    Some((
        AppInfo {
            name,
            process,
            window_title: window_title(hwnd),
        },
        Window { hwnd, thread_id },
    ))
}

fn window_title(hwnd: HWND) -> Option<String> {
    let mut buffer = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    let title = String::from_utf16_lossy(&buffer[..len.max(0) as usize]);
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

fn process_image_path(pid: u32) -> Option<String> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    result.ok()?;
    Some(String::from_utf16_lossy(&buffer[..len as usize]))
}

/// The `FileDescription` from an executable's version resource, e.g.
/// "Google Chrome" for chrome.exe.
fn file_description(path: &str) -> Option<String> {
    let path = HSTRING::from(path);
    let size = unsafe { GetFileVersionInfoSizeW(&path, None) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    unsafe { GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()) }.ok()?;

    // Look the description up in each translation the resource declares,
    // then in the common US English / Unicode block.
    let mut translations = Vec::new();
    let mut table: *mut c_void = std::ptr::null_mut();
    let mut table_len = 0u32;
    if unsafe {
        VerQueryValueW(
            data.as_ptr().cast(),
            w!("\\VarFileInfo\\Translation"),
            &mut table,
            &mut table_len,
        )
    }
    .as_bool()
    {
        let words =
            unsafe { std::slice::from_raw_parts(table as *const u16, table_len as usize / 2) };
        translations.extend(
            words
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[language, codepage]| (language, codepage)),
        );
    }
    translations.push((0x0409, 0x04B0));

    translations.into_iter().find_map(|(language, codepage)| {
        let key = HSTRING::from(format!(
            "\\StringFileInfo\\{language:04x}{codepage:04x}\\FileDescription"
        ));
        let mut value: *mut c_void = std::ptr::null_mut();
        let mut chars = 0u32;
        if !unsafe { VerQueryValueW(data.as_ptr().cast(), &key, &mut value, &mut chars) }.as_bool()
            || chars == 0
        {
            return None;
        }
        let text = unsafe { std::slice::from_raw_parts(value as *const u16, chars as usize) };
        let text = String::from_utf16_lossy(text);
        let text = text.trim_end_matches('\0').trim();
        (!text.is_empty()).then(|| text.to_string())
    })
}

pub fn focused_field(_window: &Window) -> FocusedField {
    if !ensure_mta() {
        return FocusedField::Unavailable;
    }
    match read_focused_field() {
        Ok(field) => field,
        Err(e) => {
            debug!("App context: UI Automation could not read the focused field: {e}");
            FocusedField::Unavailable
        }
    }
}

fn read_focused_field() -> windows::core::Result<FocusedField> {
    let automation: IUIAutomation2 =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)? };
    unsafe {
        automation.SetConnectionTimeout(UIA_TIMEOUT_MS)?;
        automation.SetTransactionTimeout(UIA_TIMEOUT_MS)?;
    }

    let element = unsafe { automation.GetFocusedElement()? };
    if unsafe { element.CurrentIsPassword() }.is_ok_and(|protected| protected.as_bool()) {
        return Ok(FocusedField::Protected);
    }

    if let Some((range, document)) = cursor_range(&element) {
        return read_around_cursor(&range, document.as_ref());
    }

    // Single-line controls without a text pattern (address bars, combo
    // boxes) still expose their content as a value, without a caret.
    if let Ok(pattern) =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
    {
        let value = clean_text(&unsafe { pattern.CurrentValue()? }.to_string());
        return Ok(FocusedField::Text {
            text: FocusedText {
                before_cursor: tail_chars(&value, MAX_BEFORE_CURSOR_CHARS).to_string(),
                ..Default::default()
            },
            cursor_y: None,
        });
    }

    Ok(FocusedField::Unavailable)
}

/// The selection (or the empty range at the caret when nothing is selected)
/// and the focused control's whole text range.
fn cursor_range(
    element: &IUIAutomationElement,
) -> Option<(IUIAutomationTextRange, Option<IUIAutomationTextRange>)> {
    let pattern =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
            .ok()?;
    let document = unsafe { pattern.DocumentRange() }.ok();

    let selection = unsafe { pattern.GetSelection() }.ok().and_then(|ranges| {
        (unsafe { ranges.Length() }.unwrap_or(0) > 0)
            .then(|| unsafe { ranges.GetElement(0) }.ok())
            .flatten()
    });
    if let Some(selection) = selection {
        return Some((selection, document));
    }

    // Some providers return no selection while a caret is present; only
    // TextPattern2 can report the caret on its own.
    let pattern =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id) }
            .ok()?;
    let mut is_active = BOOL::default();
    let caret = unsafe { pattern.GetCaretRange(&mut is_active) }.ok()?;
    Some((caret, document))
}

/// Keeps `range` inside `bounds`. Chromium's text ranges walk the whole web
/// page, so without this the text "before the cursor" in an empty chat box
/// would be whatever precedes it on the page (a sidebar, a header).
fn clamp_range(
    range: &IUIAutomationTextRange,
    bounds: &IUIAutomationTextRange,
) -> windows::core::Result<()> {
    unsafe {
        if range.CompareEndpoints(
            TextPatternRangeEndpoint_Start,
            bounds,
            TextPatternRangeEndpoint_Start,
        )? < 0
        {
            range.MoveEndpointByRange(
                TextPatternRangeEndpoint_Start,
                bounds,
                TextPatternRangeEndpoint_Start,
            )?;
        }
        if range.CompareEndpoints(
            TextPatternRangeEndpoint_End,
            bounds,
            TextPatternRangeEndpoint_End,
        )? > 0
        {
            range.MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                bounds,
                TextPatternRangeEndpoint_End,
            )?;
        }
    }
    Ok(())
}

fn read_around_cursor(
    range: &IUIAutomationTextRange,
    document: Option<&IUIAutomationTextRange>,
) -> windows::core::Result<FocusedField> {
    let selection = clean_text(&unsafe { range.GetText(MAX_SELECTION_CHARS as i32)? }.to_string());

    let before = unsafe { range.Clone()? };
    unsafe {
        before.MoveEndpointByRange(
            TextPatternRangeEndpoint_End,
            range,
            TextPatternRangeEndpoint_Start,
        )?;
        before.MoveEndpointByUnit(
            TextPatternRangeEndpoint_Start,
            TextUnit_Character,
            -(MAX_BEFORE_CURSOR_CHARS as i32),
        )?;
    }
    if let Some(document) = document {
        clamp_range(&before, document)?;
    }
    let before_text = clean_text(&unsafe { before.GetText(-1)? }.to_string());

    let after = unsafe { range.Clone()? };
    unsafe {
        after.MoveEndpointByRange(
            TextPatternRangeEndpoint_Start,
            range,
            TextPatternRangeEndpoint_End,
        )?;
        after.MoveEndpointByUnit(
            TextPatternRangeEndpoint_End,
            TextUnit_Character,
            MAX_AFTER_CURSOR_CHARS as i32,
        )?;
    }
    if let Some(document) = document {
        clamp_range(&after, document)?;
    }
    let after_text =
        clean_text(&unsafe { after.GetText(MAX_AFTER_CURSOR_CHARS as i32)? }.to_string());

    // An empty range usually has no bounding box, so locate the cursor from
    // the text just before it (its last line) or else just after it.
    let cursor_y = last_rect_center_y(&before).or_else(|| first_rect_center_y(&after));

    Ok(FocusedField::Text {
        text: FocusedText {
            before_cursor: tail_chars(&before_text, MAX_BEFORE_CURSOR_CHARS).to_string(),
            selection: head_chars(&selection, MAX_SELECTION_CHARS).to_string(),
            after_cursor: head_chars(&after_text, MAX_AFTER_CURSOR_CHARS).to_string(),
        },
        cursor_y,
    })
}

/// Bounding rectangles of a text range as `[left, top, width, height]`.
fn bounding_rects(range: &IUIAutomationTextRange) -> Vec<[f64; 4]> {
    let Ok(array) = (unsafe { range.GetBoundingRectangles() }) else {
        return Vec::new();
    };
    if array.is_null() {
        return Vec::new();
    }
    let values = unsafe { safearray_f64(array) };
    let _ = unsafe { SafeArrayDestroy(array) };
    values.as_chunks::<4>().0.to_vec()
}

/// Copies a one-dimensional SAFEARRAY of doubles.
unsafe fn safearray_f64(array: *const SAFEARRAY) -> Vec<f64> {
    let array = &*array;
    if array.cDims != 1 || array.cbElements != 8 || array.pvData.is_null() {
        return Vec::new();
    }
    let len = array.rgsabound[0].cElements as usize;
    std::slice::from_raw_parts(array.pvData as *const f64, len).to_vec()
}

fn last_rect_center_y(range: &IUIAutomationTextRange) -> Option<f64> {
    bounding_rects(range)
        .last()
        .map(|[_, top, _, height]| top + height / 2.0)
}

fn first_rect_center_y(range: &IUIAutomationTextRange) -> Option<f64> {
    bounding_rects(range)
        .first()
        .map(|[_, top, _, height]| top + height / 2.0)
}

pub fn screen_text(window: &Window, cursor_y: Option<f64>, max_chars: usize) -> Option<String> {
    if !ensure_mta() {
        return None;
    }
    match ocr_window(window, cursor_y, max_chars) {
        Ok(text) => text,
        Err(e) => {
            debug!("App context: OCR of the foreground window failed: {e}");
            None
        }
    }
}

fn ocr_window(
    window: &Window,
    cursor_y: Option<f64>,
    max_chars: usize,
) -> windows::core::Result<Option<String>> {
    if unsafe { IsIconic(window.hwnd) }.as_bool() {
        return Ok(None);
    }
    let Some(rect) = visible_window_rect(window.hwnd) else {
        return Ok(None);
    };
    let Some(engine) = ocr_engine() else {
        return Ok(None);
    };

    let (pixels, width, height) = capture_screen_rect(rect)?;
    let max_dimension = OcrEngine::MaxImageDimension()?.max(1) as i32;
    let scale = (width.max(height) + max_dimension - 1) / max_dimension;
    let (pixels, width, height) = downscale(pixels, width, height, scale.max(1));

    let writer = DataWriter::new()?;
    writer.WriteBytes(&pixels)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &writer.DetachBuffer()?,
        BitmapPixelFormat::Bgra8,
        width,
        height,
    )?;
    let result = engine.RecognizeAsync(&bitmap)?.get()?;

    let lines = result.Lines()?;
    let mut screen_lines = Vec::with_capacity(lines.Size()? as usize);
    for index in 0..lines.Size()? {
        let line = lines.GetAt(index)?;
        let text = repair_ocr_text(&line.Text()?.to_string());
        let words = line.Words()?;
        let y = if words.Size()? > 0 {
            let bounds = words.GetAt(0)?.BoundingRect()?;
            rect.top as f64 + (bounds.Y as f64 + bounds.Height as f64 / 2.0) * scale.max(1) as f64
        } else {
            rect.top as f64
        };
        screen_lines.push(ScreenLine { text, y });
    }

    let cursor_y = cursor_y.or_else(|| system_caret_y(window));
    let text = select_screen_lines(&screen_lines, cursor_y, max_chars);
    Ok((!text.is_empty()).then_some(text))
}

/// The OCR engine for the user's profile languages, created once.
fn ocr_engine() -> Option<&'static OcrEngine> {
    static ENGINE: OnceLock<Option<OcrEngine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| match OcrEngine::TryCreateFromUserProfileLanguages() {
            Ok(engine) => Some(engine),
            Err(e) => {
                debug!("App context: no OCR engine for the user's languages: {e}");
                None
            }
        })
        .as_ref()
}

/// The window's visible frame (without the invisible resize borders),
/// clipped to the virtual screen.
fn visible_window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    let frame = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if frame.is_err() {
        unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    }

    let (left, top, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    let clipped = RECT {
        left: rect.left.max(left),
        top: rect.top.max(top),
        right: rect.right.min(left + width),
        bottom: rect.bottom.min(top + height),
    };
    (clipped.right - clipped.left >= 16 && clipped.bottom - clipped.top >= 16).then_some(clipped)
}

/// Copies a screen rectangle as top-down, opaque BGRA pixels. The window is
/// in the foreground, so its on-screen pixels are the window's content.
fn capture_screen_rect(rect: RECT) -> windows::core::Result<(Vec<u8>, i32, i32)> {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let mut pixels = vec![0u8; width as usize * height as usize * 4];

    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(windows::core::Error::from_win32());
        }
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, bitmap.into());
        let copied = BitBlt(
            memory,
            0,
            0,
            width,
            height,
            Some(screen),
            rect.left,
            rect.top,
            SRCCOPY,
        );
        SelectObject(memory, previous);

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative height asks for top-down rows.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let rows = GetDIBits(
            memory,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );

        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);

        copied?;
        if rows != height {
            return Err(windows::core::Error::from_win32());
        }
    }

    // GDI leaves alpha at zero, which would read as fully transparent.
    for alpha in pixels.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    Ok((pixels, width, height))
}

/// Nearest-neighbour downscale by an integer factor, for windows larger than
/// the OCR engine accepts.
fn downscale(pixels: Vec<u8>, width: i32, height: i32, factor: i32) -> (Vec<u8>, i32, i32) {
    if factor <= 1 {
        return (pixels, width, height);
    }
    let (new_width, new_height) = (width / factor, height / factor);
    let mut out = Vec::with_capacity(new_width as usize * new_height as usize * 4);
    for y in 0..new_height {
        for x in 0..new_width {
            let source = ((y * factor * width + x * factor) * 4) as usize;
            out.extend_from_slice(&pixels[source..source + 4]);
        }
    }
    (out, new_width, new_height)
}

/// Vertical screen position of the system caret in the window's GUI thread.
/// Win32 edit controls and most Chromium/Electron apps keep one for screen
/// magnifiers even when they expose no text through UI Automation.
fn system_caret_y(window: &Window) -> Option<f64> {
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetGUIThreadInfo(window.thread_id, &mut info) }.ok()?;
    if info.hwndCaret.is_invalid() {
        return None;
    }
    let mut point = POINT {
        x: info.rcCaret.left,
        y: (info.rcCaret.top + info.rcCaret.bottom) / 2,
    };
    unsafe { ClientToScreen(info.hwndCaret, &mut point) }
        .as_bool()
        .then_some(point.y as f64)
}
