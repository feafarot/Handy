//! Telling what an app holding the microphone means: a call in an app made
//! for calls is a meeting for sure; a browser or anything else might be one.

use serde::{Deserialize, Serialize};
use specta::Type;

/// An app holding the microphone, as the platform reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MicApp {
    /// Stays the same across the app's updates: the executable's file name
    /// ("zoom.exe"), or a packaged app's family name, lowercased.
    pub id: String,
    /// The name the app gives itself, if the platform knows it.
    pub name: Option<String>,
    /// Titles of the app's visible windows.
    pub window_titles: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AppKind {
    /// Made for calls (Zoom, Teams): holding the microphone means a meeting.
    MeetingApp,
    /// Could be a web meeting, could be anything.
    Browser,
    Other,
}

/// An app holding the microphone that might be in a meeting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// See [`MicApp::id`].
    pub id: String,
    /// "Zoom", "Google Chrome".
    pub name: String,
    pub kind: AppKind,
    /// The web meeting service a browser window shows, e.g. "Google Meet".
    pub service: Option<&'static str>,
    /// The window title that gave the service away.
    pub hint: Option<String>,
}

impl Candidate {
    /// Holding the microphone is enough to call it a meeting.
    pub fn is_certain(&self) -> bool {
        self.kind == AppKind::MeetingApp
    }

    /// What a meeting recorded from it is held in: "Zoom", or
    /// "Google Meet (Google Chrome)" for a web meeting.
    pub fn label(&self) -> String {
        match self.service {
            Some(service) => format!("{service} ({})", self.name),
            None => self.name.clone(),
        }
    }
}

/// Apps made for calls, by executable or, ending in `_`, by package family
/// name prefix.
const MEETING_APPS: &[(&str, &str)] = &[
    ("zoom.exe", "Zoom"),
    ("ms-teams.exe", "Microsoft Teams"),
    ("msteams_", "Microsoft Teams"),
    ("teams.exe", "Microsoft Teams"),
    ("webex.exe", "Webex"),
    ("ciscocollabhost.exe", "Webex"),
    ("webexmta.exe", "Webex"),
    ("atmgr.exe", "Webex"),
    ("slack.exe", "Slack"),
    ("g2mcomm.exe", "GoTo Meeting"),
    ("goto.exe", "GoTo"),
    ("ringcentral.exe", "RingCentral"),
    ("skype.exe", "Skype"),
];

const BROWSERS: &[(&str, &str)] = &[
    ("chrome.exe", "Google Chrome"),
    ("msedge.exe", "Microsoft Edge"),
    ("firefox.exe", "Firefox"),
    ("brave.exe", "Brave"),
    ("opera.exe", "Opera"),
    ("vivaldi.exe", "Vivaldi"),
    ("arc.exe", "Arc"),
    ("thebrowsercompany.arc_", "Arc"),
    ("zen.exe", "Zen Browser"),
    ("librewolf.exe", "LibreWolf"),
    ("chromium.exe", "Chromium"),
    ("thorium.exe", "Thorium"),
    ("waterfox.exe", "Waterfox"),
    ("floorp.exe", "Floorp"),
];

/// Apps that hold the microphone to pass it on (noise removal, virtual
/// mixers): the app actually in the call shows up on its own.
const PASS_THROUGH: &[&str] = &[
    "nvcontainer.exe",
    "nvidia broadcast.exe",
    "krisp.exe",
    "voicemeeter.exe",
    "voicemeeter_x64.exe",
    "voicemeeterpro.exe",
    "voicemeeterpro_x64.exe",
    "voicemeeter8.exe",
    "voicemeeter8x64.exe",
    "steelseriessonar.exe",
    "steelseriesgg.exe",
    "audiodg.exe",
];

/// Web meeting services, by what their tab titles start with or contain
/// (lowercase).
const WEB_MEETINGS: &[(Match, &str)] = &[
    (Match::Prefix("meet - "), "Google Meet"),
    (Match::Prefix("meet – "), "Google Meet"),
    (Match::Contains("google meet"), "Google Meet"),
    (Match::Contains("microsoft teams"), "Microsoft Teams"),
    (Match::Contains("zoom meeting"), "Zoom"),
    (Match::Contains("zoom webinar"), "Zoom"),
    (Match::Contains("webex"), "Webex"),
    (Match::Contains("jitsi meet"), "Jitsi Meet"),
    (Match::Contains("whereby"), "Whereby"),
];

enum Match {
    Prefix(&'static str),
    Contains(&'static str),
}

impl Match {
    fn matches(&self, title: &str) -> bool {
        match self {
            Match::Prefix(prefix) => title.starts_with(prefix),
            Match::Contains(needle) => title.contains(needle),
        }
    }
}

fn lookup(table: &[(&str, &'static str)], id: &str) -> Option<&'static str> {
    table.iter().find_map(|&(key, name)| {
        let hit = if key.ends_with('_') {
            id.starts_with(key)
        } else {
            id == key
        };
        hit.then_some(name)
    })
}

/// Sorts out an app holding the microphone; `None` for apps that never mean
/// a meeting. `own_id` is Handy's own [`MicApp::id`].
pub fn classify(app: MicApp, own_id: &str) -> Option<Candidate> {
    if app.id == own_id || PASS_THROUGH.contains(&app.id.as_str()) {
        return None;
    }
    if let Some(name) = lookup(MEETING_APPS, &app.id) {
        return Some(Candidate {
            id: app.id,
            name: name.to_string(),
            kind: AppKind::MeetingApp,
            service: None,
            hint: None,
        });
    }
    if let Some(name) = lookup(BROWSERS, &app.id) {
        let found = app.window_titles.iter().find_map(|title| {
            let lower = title.to_lowercase();
            WEB_MEETINGS
                .iter()
                .find(|(pattern, _)| pattern.matches(&lower))
                .map(|&(_, service)| (service, title.clone()))
        });
        return Some(Candidate {
            id: app.id,
            name: name.to_string(),
            kind: AppKind::Browser,
            service: found.as_ref().map(|(service, _)| *service),
            hint: found.map(|(_, title)| title),
        });
    }
    let name = app
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| fallback_name(&app.id));
    Some(Candidate {
        id: app.id,
        name,
        kind: AppKind::Other,
        service: None,
        hint: None,
    })
}

/// "discord.exe" → "discord"; "microsoft.windowssoundrecorder_8wekyb3d8bbwe"
/// → "windowssoundrecorder".
fn fallback_name(id: &str) -> String {
    let id = id.strip_suffix(".exe").unwrap_or(id);
    let id = id.split('_').next().unwrap_or(id);
    id.rsplit('.').next().unwrap_or(id).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, titles: &[&str]) -> MicApp {
        MicApp {
            id: id.into(),
            name: None,
            window_titles: titles.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn meeting_apps_are_certain() {
        let zoom = classify(app("zoom.exe", &[]), "handy.exe").unwrap();
        assert_eq!(zoom.name, "Zoom");
        assert!(zoom.is_certain());
        let teams = classify(app("msteams_8wekyb3d8bbwe", &[]), "handy.exe").unwrap();
        assert_eq!(teams.label(), "Microsoft Teams");
        assert!(teams.is_certain());
    }

    #[test]
    fn browsers_are_uncertain_and_name_the_web_meeting() {
        let meet = classify(
            app(
                "chrome.exe",
                &[
                    "Inbox - Gmail - Google Chrome",
                    "Meet - abc-defg-hij - Google Chrome",
                ],
            ),
            "handy.exe",
        )
        .unwrap();
        assert!(!meet.is_certain());
        assert_eq!(meet.service, Some("Google Meet"));
        assert_eq!(
            meet.hint.as_deref(),
            Some("Meet - abc-defg-hij - Google Chrome")
        );
        assert_eq!(meet.label(), "Google Meet (Google Chrome)");

        let unknown = classify(app("firefox.exe", &["Some page"]), "handy.exe").unwrap();
        assert_eq!(unknown.kind, AppKind::Browser);
        assert_eq!(unknown.service, None);
        assert_eq!(unknown.label(), "Firefox");
    }

    #[test]
    fn handy_and_audio_tools_are_ignored() {
        assert_eq!(classify(app("handy.exe", &[]), "handy.exe"), None);
        assert_eq!(classify(app("nvcontainer.exe", &[]), "handy.exe"), None);
    }

    #[test]
    fn other_apps_use_their_own_name_or_one_from_the_id() {
        let mut discord = app("discord.exe", &[]);
        discord.name = Some("Discord".into());
        let discord = classify(discord, "handy.exe").unwrap();
        assert_eq!(discord.kind, AppKind::Other);
        assert_eq!(discord.name, "Discord");
        let recorder = classify(
            app("microsoft.windowssoundrecorder_8wekyb3d8bbwe", &[]),
            "handy.exe",
        )
        .unwrap();
        assert_eq!(recorder.name, "windowssoundrecorder");
    }
}
