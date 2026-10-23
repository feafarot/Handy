//! Learns spellings from the user correcting a dictation right after it was
//! pasted.
//!
//! After a paste, the focused field is read through the accessibility API
//! and watched for a while. Whenever its text changes and then settles, the
//! pasted text is diffed against what it became (`learner::extract_corrections`)
//! and the replaced words are handed to the dictionary. Nothing but those
//! words leaves this module; the field's text is never stored.
//!
//! Platform support: Windows (UI Automation, see `windows.rs`). Another
//! platform plugs in a `FieldReader`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{debug, warn};
use tauri::{AppHandle, Manager};

use super::{learner, DictionaryManager, DictionarySource};

/// The focused text field as captured right after the paste.
pub trait FocusedField {
    /// The field's current text, or `None` once it can no longer be read
    /// (the control went away, the app stopped answering).
    fn text(&self) -> Option<String>;
    /// What to call the field's app in the review list.
    fn origin(&self) -> String;
}

/// Captures the field that has keyboard focus.
pub trait FieldReader {
    fn capture(&self) -> Option<Box<dyn FocusedField>>;
}

/// How watching a pasted dictation is paced.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Wait for the paste to land before the first read.
    pub settle: Duration,
    /// Re-read the field this often.
    pub poll: Duration,
    /// Polls without a change after an edit before the diff runs.
    pub stable_polls: u32,
    /// Stop watching after this long.
    pub max_watch: Duration,
    /// Give the paste this many reads to show up in the field.
    pub land_retries: u32,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            settle: Duration::from_millis(500),
            poll: Duration::from_millis(500),
            stable_polls: 3,
            max_watch: Duration::from_secs(90),
            land_retries: 4,
        }
    }
}

/// Whether this platform can read the focused field.
pub fn supported() -> bool {
    cfg!(target_os = "windows")
}

#[cfg(target_os = "windows")]
fn reader() -> Option<Box<dyn FieldReader>> {
    Some(Box::new(super::windows::UiaFieldReader))
}

#[cfg(not(target_os = "windows"))]
fn reader() -> Option<Box<dyn FieldReader>> {
    None
}

/// Watches the field `pasted` just went into, on its own thread, until the
/// text stops changing for good, `still_wanted` says no (a new dictation
/// started), or the watch times out.
pub fn start(app: AppHandle, pasted: String, still_wanted: impl Fn() -> bool + Send + 'static) {
    if !supported() || pasted.trim().is_empty() {
        return;
    }
    let Some(dictionary) = app.try_state::<Arc<DictionaryManager>>() else {
        return;
    };
    let dictionary = Arc::clone(&dictionary);
    let spawned = std::thread::Builder::new()
        .name("dictionary-edit-monitor".into())
        .spawn(move || {
            let Some(reader) = reader() else {
                return;
            };
            let timing = Timing::default();
            std::thread::sleep(timing.settle);
            let Some(field) = reader.capture() else {
                debug!("Dictionary: no readable field to watch after the paste");
                return;
            };
            let known = dictionary.active_words().unwrap_or_default();
            let origin = field.origin();
            watch(
                field.as_ref(),
                &pasted,
                &known,
                timing,
                still_wanted,
                std::thread::sleep,
                |words| {
                    if let Err(e) = dictionary.suggest(&words, DictionarySource::Learned, &origin) {
                        warn!("Dictionary: saving learned words failed: {e:#}");
                    }
                },
            );
        });
    if let Err(e) = spawned {
        warn!("Dictionary: starting the edit monitor failed: {e}");
    }
}

/// The watch loop, free of threads and clocks so it can be tested. Calls
/// `on_learned` with each batch of corrected words as they are found.
pub fn watch(
    field: &dyn FocusedField,
    pasted: &str,
    known: &[String],
    timing: Timing,
    mut still_wanted: impl FnMut() -> bool,
    mut sleep: impl FnMut(Duration),
    mut on_learned: impl FnMut(Vec<String>),
) {
    // The paste may still be landing: give the field a few reads to show it.
    let mut landed = None;
    for attempt in 0..=timing.land_retries {
        if attempt > 0 {
            sleep(timing.poll);
        }
        let Some(text) = field.text() else {
            debug!("Dictionary: the pasted field cannot be read");
            return;
        };
        if text.contains(pasted.trim()) {
            landed = Some(text);
            break;
        }
    }
    let Some(mut last) = landed else {
        debug!("Dictionary: the pasted text did not show up in the focused field");
        return;
    };

    let mut known: Vec<String> = known.to_vec();
    let mut changed = false;
    let mut stable = 0u32;
    let started = Instant::now();
    let mut extract = |text: &str, known: &mut Vec<String>| {
        let words = learner::extract_corrections(pasted, text, known);
        if !words.is_empty() {
            known.extend(words.iter().cloned());
            on_learned(words);
        }
    };

    while started.elapsed() < timing.max_watch && still_wanted() {
        sleep(timing.poll);
        let Some(text) = field.text() else {
            break;
        };
        if text != last {
            last = text;
            changed = true;
            stable = 0;
        } else if changed {
            stable += 1;
            if stable >= timing.stable_polls {
                extract(&last, &mut known);
                changed = false;
            }
        }
    }
    if changed {
        extract(&last, &mut known);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A field whose text follows a script, one entry per read.
    struct Scripted {
        reads: RefCell<Vec<Option<String>>>,
    }

    impl Scripted {
        fn new(reads: &[Option<&str>]) -> Self {
            Self {
                reads: RefCell::new(reads.iter().rev().map(|r| r.map(str::to_string)).collect()),
            }
        }
    }

    impl FocusedField for Scripted {
        fn text(&self) -> Option<String> {
            let mut reads = self.reads.borrow_mut();
            match reads.len() {
                0 => None,
                1 => reads[0].clone(),
                _ => reads.pop().flatten(),
            }
        }

        fn origin(&self) -> String {
            "test".into()
        }
    }

    fn timing() -> Timing {
        Timing {
            settle: Duration::ZERO,
            poll: Duration::ZERO,
            stable_polls: 2,
            max_watch: Duration::from_secs(5),
            land_retries: 2,
        }
    }

    #[test]
    fn learns_once_the_edit_settles() {
        let field = Scripted::new(&[
            Some("hi Shunade"),
            Some("hi Shunade"),
            Some("hi Sin"),
            Some("hi Sinead"),
            Some("hi Sinead"),
            Some("hi Sinead"),
            None,
        ]);
        let mut learned = Vec::new();
        watch(
            &field,
            "hi Shunade",
            &[],
            timing(),
            || true,
            |_| {},
            |words| learned.push(words),
        );
        assert_eq!(learned, vec![vec!["Sinead".to_string()]]);
    }

    #[test]
    fn stops_when_no_longer_wanted() {
        let field = Scripted::new(&[Some("hi Shunade"), Some("hi Sinead")]);
        let mut learned = Vec::new();
        let mut polls = 0;
        watch(
            &field,
            "hi Shunade",
            &[],
            timing(),
            || {
                polls += 1;
                polls < 3
            },
            |_| {},
            |words| learned.push(words),
        );
        // The last read happened before the stop; the final diff still runs.
        assert_eq!(learned, vec![vec!["Sinead".to_string()]]);
    }

    #[test]
    fn gives_up_when_the_paste_never_lands() {
        let field = Scripted::new(&[Some("unrelated"), Some("unrelated"), Some("unrelated")]);
        let mut learned = Vec::new();
        watch(
            &field,
            "hi Shunade",
            &[],
            timing(),
            || true,
            |_| {},
            |words| learned.push(words),
        );
        assert!(learned.is_empty());
    }
}
