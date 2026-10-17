//! Deciding, from which apps hold the microphone over time, when a meeting
//! starts and ends. No I/O: `super` feeds it what it sees every poll and
//! carries out what it returns.
//!
//! An app holding the microphone without a break is one *session*. Each
//! session leads to at most one meeting or one question, so a meeting
//! stopped by hand, or a question answered "not now", does not come back
//! while the call goes on. A short break (a reconnect, a device switch) does
//! not end a session.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use super::apps::Candidate;
use crate::settings::{MeetingAutoStart, MeetingAutoStop};

/// How long an app made for calls holds the microphone before it counts:
/// long enough to skip a device check.
const CERTAIN_DELAY: Duration = Duration::from_secs(3);
/// The same for other apps, long enough to skip a voice message or a short
/// dictation.
const UNCERTAIN_DELAY: Duration = Duration::from_secs(10);
/// How long an app may let go of the microphone within one session.
const SESSION_GAP: Duration = Duration::from_secs(30);
/// How long a detected meeting's app has let go of the microphone before the
/// meeting counts as over.
const STOP_DELAY: Duration = Duration::from_secs(60);
/// How long the notice that a recording started stays up.
const NOTICE_TIME: Duration = Duration::from_secs(15);

/// The settings that drive detection.
pub struct Policy {
    pub mode: MeetingAutoStart,
    pub auto_stop: MeetingAutoStop,
    /// Ids of the apps full-auto leaves alone.
    pub blocklist: HashSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeetingPhase {
    Idle,
    Recording(i64),
    /// Done recording, still transcribing.
    Finishing,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Record a meeting held in this app; `automatic` when nobody was asked.
    Start {
        candidate: Candidate,
        automatic: bool,
    },
    Stop {
        meeting_id: i64,
    },
    /// Leave this app alone from now on.
    Block {
        id: String,
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptKind {
    /// Whether to record a possible meeting.
    Start {
        candidate: Candidate,
        /// The answer may block the app; only full-auto uses the blocklist.
        can_block: bool,
    },
    /// A recording started without asking. Blocking the app also stops it.
    Started {
        meeting_id: i64,
        candidate: Candidate,
    },
    /// Whether to stop a meeting whose app let go of the microphone.
    Stop { meeting_id: i64, app: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub id: u32,
    pub kind: PromptKind,
    shown_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Record,
    Stop,
    Block,
    Dismiss,
}

struct Session {
    candidate: Candidate,
    first_seen: Instant,
    last_seen: Instant,
    /// Already led to a meeting or a question (or was ignored).
    handled: bool,
}

/// A meeting recorded because it was detected.
struct Detected {
    meeting_id: i64,
    app_id: String,
    app: String,
    released_at: Option<Instant>,
    /// What to do about the app letting go was done for this release.
    settled: bool,
}

#[derive(Default)]
pub struct Detector {
    sessions: HashMap<String, Session>,
    detected: Option<Detected>,
    prompt: Option<Prompt>,
    next_prompt_id: u32,
}

impl Detector {
    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    /// Whether a detected meeting is being recorded, which needs watching
    /// even when detection is off.
    pub fn is_watching(&self) -> bool {
        self.detected.is_some()
    }

    /// The app most likely in a meeting right now, among those holding the
    /// microphone.
    pub fn best(candidates: &[Candidate]) -> Option<&Candidate> {
        candidates
            .iter()
            .find(|c| c.is_certain())
            .or_else(|| candidates.iter().find(|c| c.service.is_some()))
            .or_else(|| candidates.first())
    }

    /// Takes in the apps holding the microphone right now.
    pub fn tick(
        &mut self,
        now: Instant,
        holding: Vec<Candidate>,
        meeting: MeetingPhase,
        policy: &Policy,
    ) -> Vec<Action> {
        for candidate in holding {
            self.sessions
                .entry(candidate.id.clone())
                .and_modify(|session| {
                    session.last_seen = now;
                    session.candidate = candidate.clone();
                })
                .or_insert(Session {
                    candidate,
                    first_seen: now,
                    last_seen: now,
                    handled: false,
                });
        }
        self.sessions
            .retain(|_, session| now.duration_since(session.last_seen) < SESSION_GAP);

        let mut actions = Vec::new();
        self.watch_detected(now, meeting, policy, &mut actions);
        if let Some(Prompt {
            kind: PromptKind::Started { .. },
            shown_at,
            ..
        }) = &self.prompt
        {
            if now.duration_since(*shown_at) >= NOTICE_TIME {
                self.prompt = None;
            }
        }

        if meeting != MeetingPhase::Idle {
            // Whatever holds the microphone now belongs to this meeting.
            for session in self.sessions.values_mut() {
                session.handled = true;
            }
            if self.start_prompt().is_some() {
                self.prompt = None;
            }
            return actions;
        }

        if let Some(candidate) = self.start_prompt() {
            let gone = !self.holds(&candidate.id, now);
            if gone || policy.mode == MeetingAutoStart::Manual {
                self.prompt = None;
            }
        }
        if policy.mode == MeetingAutoStart::Manual || self.prompt.is_some() {
            return actions;
        }

        let mut ready: Vec<&mut Session> = self
            .sessions
            .values_mut()
            .filter(|session| {
                let delay = if session.candidate.is_certain() {
                    CERTAIN_DELAY
                } else {
                    UNCERTAIN_DELAY
                };
                !session.handled
                    && session.last_seen == now
                    && now.duration_since(session.first_seen) >= delay
            })
            .collect();
        ready.sort_by_key(|session| (!session.candidate.is_certain(), session.first_seen));
        let full = policy.mode == MeetingAutoStart::Full;
        let mut chosen = None;
        for session in ready {
            session.handled = true;
            if !(full && policy.blocklist.contains(&session.candidate.id)) {
                chosen = Some(session.candidate.clone());
                break;
            }
        }
        let Some(candidate) = chosen else {
            return actions;
        };
        if full && candidate.is_certain() {
            actions.push(Action::Start {
                candidate,
                automatic: true,
            });
        } else {
            self.show(
                PromptKind::Start {
                    candidate,
                    can_block: full,
                },
                now,
            );
        }
        actions
    }

    /// Notes that a meeting started for `candidate`, as asked or on its own.
    pub fn started(
        &mut self,
        meeting_id: i64,
        candidate: &Candidate,
        automatic: bool,
        now: Instant,
    ) {
        self.detected = Some(Detected {
            meeting_id,
            app_id: candidate.id.clone(),
            app: candidate.label(),
            released_at: None,
            settled: false,
        });
        if automatic {
            self.show(
                PromptKind::Started {
                    meeting_id,
                    candidate: candidate.clone(),
                },
                now,
            );
        }
    }

    /// The user's answer to prompt `id`; stale answers are ignored.
    pub fn answer(&mut self, id: u32, answer: Answer) -> Vec<Action> {
        let Some(prompt) = self.prompt.take_if(|prompt| prompt.id == id) else {
            return Vec::new();
        };
        match (prompt.kind, answer) {
            (PromptKind::Start { candidate, .. }, Answer::Record) => vec![Action::Start {
                candidate,
                automatic: false,
            }],
            (
                PromptKind::Start {
                    candidate,
                    can_block: true,
                },
                Answer::Block,
            ) => vec![Action::Block {
                id: candidate.id,
                name: candidate.name,
            }],
            (
                PromptKind::Started {
                    meeting_id,
                    candidate,
                },
                Answer::Block,
            ) => vec![
                Action::Block {
                    id: candidate.id,
                    name: candidate.name,
                },
                Action::Stop { meeting_id },
            ],
            (
                PromptKind::Started { meeting_id, .. } | PromptKind::Stop { meeting_id, .. },
                Answer::Stop,
            ) => vec![Action::Stop { meeting_id }],
            (_, Answer::Dismiss) => Vec::new(),
            (kind, _) => {
                // An answer the prompt does not offer.
                self.prompt = Some(Prompt { kind, ..prompt });
                Vec::new()
            }
        }
    }

    fn watch_detected(
        &mut self,
        now: Instant,
        meeting: MeetingPhase,
        policy: &Policy,
        actions: &mut Vec<Action>,
    ) {
        let Some(detected) = &self.detected else {
            return;
        };
        if meeting != MeetingPhase::Recording(detected.meeting_id) {
            // Stopped, by hand or not.
            self.detected = None;
            if matches!(
                self.prompt,
                Some(Prompt {
                    kind: PromptKind::Started { .. } | PromptKind::Stop { .. },
                    ..
                })
            ) {
                self.prompt = None;
            }
            return;
        }
        let holding = self.holds(&detected.app_id, now);
        let Some(detected) = &mut self.detected else {
            return;
        };
        if holding {
            detected.released_at = None;
            detected.settled = false;
            if matches!(
                self.prompt,
                Some(Prompt {
                    kind: PromptKind::Stop { .. },
                    ..
                })
            ) {
                self.prompt = None;
            }
            return;
        }
        let released_at = *detected.released_at.get_or_insert(now);
        if detected.settled || now.duration_since(released_at) < STOP_DELAY {
            return;
        }
        detected.settled = true;
        let (meeting_id, app) = (detected.meeting_id, detected.app.clone());
        match policy.auto_stop {
            MeetingAutoStop::Stop => actions.push(Action::Stop { meeting_id }),
            MeetingAutoStop::Ask => self.show(PromptKind::Stop { meeting_id, app }, now),
            MeetingAutoStop::Off => {}
        }
    }

    fn holds(&self, app_id: &str, now: Instant) -> bool {
        self.sessions
            .get(app_id)
            .is_some_and(|session| session.last_seen == now)
    }

    fn start_prompt(&self) -> Option<&Candidate> {
        match &self.prompt {
            Some(Prompt {
                kind: PromptKind::Start { candidate, .. },
                ..
            }) => Some(candidate),
            _ => None,
        }
    }

    fn show(&mut self, kind: PromptKind, now: Instant) {
        self.next_prompt_id = self.next_prompt_id.wrapping_add(1);
        self.prompt = Some(Prompt {
            id: self.next_prompt_id,
            kind,
            shown_at: now,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::apps::AppKind;
    use super::*;

    fn candidate(id: &str, kind: AppKind) -> Candidate {
        Candidate {
            id: id.into(),
            name: id.trim_end_matches(".exe").into(),
            kind,
            service: None,
            hint: None,
        }
    }

    fn zoom() -> Candidate {
        candidate("zoom.exe", AppKind::MeetingApp)
    }

    fn chrome() -> Candidate {
        candidate("chrome.exe", AppKind::Browser)
    }

    fn policy(mode: MeetingAutoStart, auto_stop: MeetingAutoStop) -> Policy {
        Policy {
            mode,
            auto_stop,
            blocklist: HashSet::new(),
        }
    }

    /// Drives a detector second by second.
    struct Clock {
        start: Instant,
        detector: Detector,
    }

    impl Clock {
        fn new() -> Self {
            Self {
                start: Instant::now(),
                detector: Detector::default(),
            }
        }

        fn at(&self, secs: u64) -> Instant {
            self.start + Duration::from_secs(secs)
        }

        /// Ticks every second over `from..=to` and collects the actions.
        fn run(
            &mut self,
            from: u64,
            to: u64,
            holding: &[Candidate],
            meeting: MeetingPhase,
            policy: &Policy,
        ) -> Vec<Action> {
            (from..=to)
                .flat_map(|secs| {
                    let now = self.at(secs);
                    self.detector.tick(now, holding.to_vec(), meeting, policy)
                })
                .collect()
        }

        fn prompt_kind(&self) -> Option<&PromptKind> {
            self.detector.prompt().map(|prompt| &prompt.kind)
        }
    }

    #[test]
    fn full_auto_records_meeting_apps_once_they_hold_the_mic() {
        let policy = policy(MeetingAutoStart::Full, MeetingAutoStop::Stop);
        let mut clock = Clock::new();
        assert!(clock
            .run(0, 2, &[zoom()], MeetingPhase::Idle, &policy)
            .is_empty());
        assert_eq!(
            clock.run(3, 3, &[zoom()], MeetingPhase::Idle, &policy),
            [Action::Start {
                candidate: zoom(),
                automatic: true
            }]
        );
        clock.detector.started(7, &zoom(), true, clock.at(3));
        assert!(matches!(
            clock.prompt_kind(),
            Some(PromptKind::Started { meeting_id: 7, .. })
        ));
        // The notice goes away on its own.
        clock.run(4, 20, &[zoom()], MeetingPhase::Recording(7), &policy);
        assert_eq!(clock.prompt_kind(), None);
    }

    #[test]
    fn blocking_an_app_from_the_notice_also_stops_its_recording() {
        let policy = policy(MeetingAutoStart::Full, MeetingAutoStop::Stop);
        let mut clock = Clock::new();
        clock.run(0, 3, &[zoom()], MeetingPhase::Idle, &policy);
        clock.detector.started(8, &zoom(), true, clock.at(3));
        let id = clock.detector.prompt().unwrap().id;
        assert_eq!(
            clock.detector.answer(id, Answer::Block),
            [
                Action::Block {
                    id: "zoom.exe".into(),
                    name: "zoom".into()
                },
                Action::Stop { meeting_id: 8 }
            ]
        );
    }

    #[test]
    fn full_auto_asks_about_browsers_and_can_block_them() {
        let policy = policy(MeetingAutoStart::Full, MeetingAutoStop::Stop);
        let mut clock = Clock::new();
        assert!(clock
            .run(0, 12, &[chrome()], MeetingPhase::Idle, &policy)
            .is_empty());
        let prompt = clock.detector.prompt().unwrap().clone();
        assert_eq!(
            prompt.kind,
            PromptKind::Start {
                candidate: chrome(),
                can_block: true
            }
        );
        assert_eq!(
            clock.detector.answer(prompt.id, Answer::Block),
            [Action::Block {
                id: "chrome.exe".into(),
                name: "chrome".into()
            }]
        );
        assert_eq!(clock.prompt_kind(), None);
    }

    #[test]
    fn full_auto_leaves_blocked_apps_alone() {
        let mut policy = policy(MeetingAutoStart::Full, MeetingAutoStop::Stop);
        policy.blocklist.insert("zoom.exe".into());
        let mut clock = Clock::new();
        assert!(clock
            .run(0, 30, &[zoom()], MeetingPhase::Idle, &policy)
            .is_empty());
        assert_eq!(clock.prompt_kind(), None);
    }

    #[test]
    fn semi_auto_always_asks_and_ignores_the_blocklist() {
        let mut policy = policy(MeetingAutoStart::Semi, MeetingAutoStop::Stop);
        policy.blocklist.insert("zoom.exe".into());
        let mut clock = Clock::new();
        assert!(clock
            .run(0, 5, &[zoom()], MeetingPhase::Idle, &policy)
            .is_empty());
        let prompt = clock.detector.prompt().unwrap().clone();
        assert_eq!(
            prompt.kind,
            PromptKind::Start {
                candidate: zoom(),
                can_block: false
            }
        );
        // Semi-auto offers no blocking.
        assert!(clock.detector.answer(prompt.id, Answer::Block).is_empty());
        assert_eq!(
            clock.detector.answer(prompt.id, Answer::Record),
            [Action::Start {
                candidate: zoom(),
                automatic: false
            }]
        );
    }

    #[test]
    fn a_dismissed_question_is_not_asked_again_in_the_same_session() {
        let policy = policy(MeetingAutoStart::Semi, MeetingAutoStop::Stop);
        let mut clock = Clock::new();
        clock.run(0, 12, &[chrome()], MeetingPhase::Idle, &policy);
        let id = clock.detector.prompt().unwrap().id;
        clock.detector.answer(id, Answer::Dismiss);
        // A short break is the same session.
        clock.run(13, 30, &[], MeetingPhase::Idle, &policy);
        clock.run(31, 60, &[chrome()], MeetingPhase::Idle, &policy);
        assert_eq!(clock.prompt_kind(), None);
        // A long one starts a new session.
        clock.run(61, 100, &[], MeetingPhase::Idle, &policy);
        clock.run(101, 115, &[chrome()], MeetingPhase::Idle, &policy);
        assert!(matches!(
            clock.prompt_kind(),
            Some(PromptKind::Start { .. })
        ));
    }

    #[test]
    fn a_question_goes_away_with_its_app() {
        let policy = policy(MeetingAutoStart::Semi, MeetingAutoStop::Stop);
        let mut clock = Clock::new();
        clock.run(0, 12, &[chrome()], MeetingPhase::Idle, &policy);
        assert!(clock.prompt_kind().is_some());
        clock.run(13, 13, &[], MeetingPhase::Idle, &policy);
        assert_eq!(clock.prompt_kind(), None);
    }

    #[test]
    fn short_mic_use_is_ignored() {
        let policy = policy(MeetingAutoStart::Full, MeetingAutoStop::Stop);
        let mut clock = Clock::new();
        clock.run(0, 8, &[chrome()], MeetingPhase::Idle, &policy);
        clock.run(9, 50, &[], MeetingPhase::Idle, &policy);
        assert_eq!(clock.prompt_kind(), None);
    }

    #[test]
    fn manual_mode_does_nothing() {
        let policy = policy(MeetingAutoStart::Manual, MeetingAutoStop::Stop);
        let mut clock = Clock::new();
        assert!(clock
            .run(0, 30, &[zoom(), chrome()], MeetingPhase::Idle, &policy)
            .is_empty());
        assert_eq!(clock.prompt_kind(), None);
    }

    #[test]
    fn apps_in_a_meeting_recorded_by_hand_are_left_alone_after_it() {
        let policy = policy(MeetingAutoStart::Full, MeetingAutoStop::Stop);
        let mut clock = Clock::new();
        assert!(clock
            .run(0, 20, &[zoom()], MeetingPhase::Recording(1), &policy)
            .is_empty());
        assert!(clock
            .run(21, 40, &[zoom()], MeetingPhase::Idle, &policy)
            .is_empty());
    }

    #[test]
    fn detected_meetings_stop_once_their_app_lets_go() {
        let policy = policy(MeetingAutoStart::Full, MeetingAutoStop::Stop);
        let mut clock = Clock::new();
        clock.run(0, 3, &[zoom()], MeetingPhase::Idle, &policy);
        clock.detector.started(1, &zoom(), true, clock.at(3));
        clock.run(4, 100, &[zoom()], MeetingPhase::Recording(1), &policy);
        // A break shorter than the stop delay keeps it going.
        assert!(clock
            .run(101, 140, &[], MeetingPhase::Recording(1), &policy)
            .is_empty());
        clock.run(141, 141, &[zoom()], MeetingPhase::Recording(1), &policy);
        assert_eq!(
            clock.run(142, 260, &[], MeetingPhase::Recording(1), &policy),
            [Action::Stop { meeting_id: 1 }]
        );
    }

    #[test]
    fn asking_to_stop_and_keeping_the_recording() {
        let policy = policy(MeetingAutoStart::Semi, MeetingAutoStop::Ask);
        let mut clock = Clock::new();
        clock.run(0, 5, &[zoom()], MeetingPhase::Idle, &policy);
        let id = clock.detector.prompt().unwrap().id;
        clock.detector.answer(id, Answer::Record);
        clock.detector.started(2, &zoom(), false, clock.at(5));
        assert_eq!(clock.prompt_kind(), None);

        assert!(clock
            .run(6, 70, &[], MeetingPhase::Recording(2), &policy)
            .is_empty());
        let prompt = clock.detector.prompt().unwrap().clone();
        assert_eq!(
            prompt.kind,
            PromptKind::Stop {
                meeting_id: 2,
                app: "zoom".into()
            }
        );
        clock.detector.answer(prompt.id, Answer::Dismiss);
        // Not asked again while the app stays away.
        clock.run(71, 300, &[], MeetingPhase::Recording(2), &policy);
        assert_eq!(clock.prompt_kind(), None);
    }

    #[test]
    fn the_stop_question_goes_away_when_the_app_comes_back() {
        let policy = policy(MeetingAutoStart::Full, MeetingAutoStop::Ask);
        let mut clock = Clock::new();
        clock.detector.started(3, &zoom(), false, clock.at(0));
        clock.run(0, 61, &[], MeetingPhase::Recording(3), &policy);
        assert!(matches!(clock.prompt_kind(), Some(PromptKind::Stop { .. })));
        clock.run(62, 62, &[zoom()], MeetingPhase::Recording(3), &policy);
        assert_eq!(clock.prompt_kind(), None);
    }

    #[test]
    fn auto_stop_off_keeps_recording() {
        let policy = policy(MeetingAutoStart::Full, MeetingAutoStop::Off);
        let mut clock = Clock::new();
        clock.detector.started(4, &zoom(), false, clock.at(0));
        assert!(clock
            .run(0, 300, &[], MeetingPhase::Recording(4), &policy)
            .is_empty());
        assert_eq!(clock.prompt_kind(), None);
    }

    #[test]
    fn meeting_apps_go_first() {
        let candidates = [chrome(), zoom()];
        assert_eq!(Detector::best(&candidates), Some(&zoom()));
        assert_eq!(Detector::best(&[]), None);
    }
}
