//! Meeting notes, written by the LLM set up for post-processing from the
//! transcript with the speakers' names.
//!
//! Local servers often run with a few thousand tokens of context and
//! OpenAI-compatible endpoints cannot raise it per request, so a long
//! transcript is never sent whole: it is cut into parts, each part condensed
//! on its own, and the condensed parts merged into the final notes.

use anyhow::{anyhow, Result};
use log::info;
use tauri::AppHandle;

use super::store::{MeetingDetail, MeetingSegment, MeetingStore, Source};
use crate::settings::{
    get_settings, AppSettings, PostProcessProvider, APPLE_INTELLIGENCE_PROVIDER_ID,
};

/// The most transcript (or condensed notes) sent in one request: about 3k
/// tokens of English, leaving room for the instructions and the answer in a
/// 4k context.
const MAX_REQUEST_CHARS: usize = 10_000;

const NOTES_FORMAT: &str = "Write the notes in Markdown with these sections, leaving out any that would be empty:

## Summary
Two to four sentences: what the meeting was about and what came out of it.

## Discussion
Bullet points for the main topics, with the key facts, numbers and arguments.

## Decisions
Bullet points.

## Action items
One bullet per task: \"- **Owner**: task (due date if one was given)\".

## Open questions
Bullet points.

Use the speakers' names as the transcript gives them; \"Me\" is the person who recorded the meeting. \
Write in the language the meeting was held in. Leave out small talk. \
Do not add anything the transcript does not say. Reply with the notes only.";

const CONDENSE: &str = "You take notes on part of a meeting. From the text below, keep everything worth \
having in the final meeting notes: topics with their key facts and numbers, decisions, action items \
(who, what, by when) and open questions. Use short bullet points and the speakers' names as given; \
\"Me\" is the person who recorded the meeting. Write in the language the meeting was held in. \
Do not add anything the text does not say. Reply with the bullet points only.";

/// Progress of one notes request out of the expected total.
pub struct Progress {
    pub done: u32,
    pub total: u32,
}

/// The LLM the notes are written with: the one post-processing uses.
pub struct Llm {
    provider: PostProcessProvider,
    model: String,
    api_key: String,
    disable_reasoning: bool,
}

impl Llm {
    pub fn from_settings(settings: &AppSettings) -> Result<Self> {
        let provider = settings
            .active_post_process_provider()
            .cloned()
            .ok_or_else(|| anyhow!("no post-processing provider is set up"))?;
        if provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
            return Err(anyhow!("Apple Intelligence cannot write meeting notes"));
        }
        let model = settings
            .post_process_models
            .get(&provider.id)
            .cloned()
            .unwrap_or_default();
        if model.trim().is_empty() {
            return Err(anyhow!(
                "no model is set for the post-processing provider {}",
                provider.label
            ));
        }
        Ok(Self {
            api_key: settings
                .post_process_api_keys
                .get(&provider.id)
                .cloned()
                .unwrap_or_default(),
            // Same reasoning as post-processing: these accept turning it off,
            // and notes don't need it.
            disable_reasoning: matches!(provider.id.as_str(), "custom" | "openrouter"),
            provider,
            model,
        })
    }

    fn ask(&self, instructions: &str, text: String) -> Result<String> {
        let reply = tauri::async_runtime::block_on(crate::llm_client::send_chat_completion(
            &self.provider,
            self.api_key.clone(),
            &self.model,
            text,
            Some(instructions.to_string()),
            self.disable_reasoning,
        ))
        .map_err(|e| anyhow!(e))?
        .ok_or_else(|| anyhow!("the model returned nothing"))?;
        let reply =
            crate::actions::strip_invisible_chars(crate::actions::strip_think_block(&reply));
        let reply = reply.trim();
        if reply.is_empty() {
            return Err(anyhow!("the model returned nothing"));
        }
        Ok(reply.to_string())
    }
}

/// Writes and stores the notes of a finished meeting.
pub fn write(
    app: &AppHandle,
    store: &MeetingStore,
    meeting_id: i64,
    on_progress: impl Fn(Progress),
) -> Result<()> {
    let llm = Llm::from_settings(&get_settings(app))?;
    let detail = store
        .get(meeting_id)?
        .ok_or_else(|| anyhow!("no meeting {meeting_id}"))?;
    let transcript = transcript_lines(&detail);
    if transcript.is_empty() {
        return Err(anyhow!("nothing was transcribed in this meeting"));
    }

    let mut parts = pack(&transcript, MAX_REQUEST_CHARS);
    let mut done = 0;
    let mut total = parts.len() as u32 + u32::from(parts.len() > 1);
    // Condense until everything fits in one request.
    while parts.len() > 1 {
        let mut condensed = Vec::with_capacity(parts.len());
        for part in parts {
            on_progress(Progress { done, total });
            condensed.push(llm.ask(CONDENSE, part)?);
            done += 1;
        }
        parts = pack(&condensed, MAX_REQUEST_CHARS);
        if parts.len() > 1 {
            total += parts.len() as u32;
        }
    }
    on_progress(Progress { done, total });
    let source = parts.pop().unwrap_or_default();
    let notes = llm.ask(
        &format!("You write meeting notes from a meeting transcript or from notes taken on its parts.\n\n{NOTES_FORMAT}"),
        source,
    )?;
    store.set_notes(meeting_id, &notes)?;
    info!(
        "Meeting {meeting_id}: notes written in {} request(s)",
        done + 1
    );
    Ok(())
}

/// Who said a segment, as the LLM should read it.
fn speaker_name(detail: &MeetingDetail, segment: &MeetingSegment) -> String {
    match (segment.source, segment.speaker) {
        (Source::Mic, _) => "Me".to_string(),
        (Source::System, None) => "Others".to_string(),
        (Source::System, Some(speaker)) => detail
            .speakers
            .iter()
            .find(|s| s.speaker == speaker)
            .and_then(|s| s.name.clone())
            .unwrap_or_else(|| format!("Speaker {speaker}")),
    }
}

/// The transcript as `[m:ss] Name: text` lines, one per turn.
fn transcript_lines(detail: &MeetingDetail) -> Vec<String> {
    let mut segments: Vec<&MeetingSegment> = detail.segments.iter().collect();
    segments.sort_by_key(|s| (s.start_ms, s.id));
    let mut turns: Vec<(String, i64, String)> = Vec::new();
    for segment in segments {
        let name = speaker_name(detail, segment);
        match turns.last_mut() {
            Some((last, _, text)) if *last == name => {
                text.push(' ');
                text.push_str(segment.text.trim());
            }
            _ => turns.push((name, segment.start_ms, segment.text.trim().to_string())),
        }
    }
    turns
        .into_iter()
        .map(|(name, start_ms, text)| format!("[{}] {name}: {text}", offset(start_ms)))
        .collect()
}

fn offset(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Joins lines into as few requests of at most `max_chars` as keeps them
/// whole; a single longer line is cut at a word.
fn pack(lines: &[String], max_chars: usize) -> Vec<String> {
    let mut requests: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in lines {
        let mut line = line.as_str();
        while line.len() > max_chars {
            let mut cut = max_chars;
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            let cut = line[..cut].rfind(' ').filter(|&i| i > 0).unwrap_or(cut);
            if !current.is_empty() {
                requests.push(std::mem::take(&mut current));
            }
            requests.push(line[..cut].to_string());
            line = line[cut..].trim_start();
        }
        if !current.is_empty() && current.len() + 1 + line.len() > max_chars {
            requests.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        requests.push(current);
    }
    requests
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::store::{Meeting, MeetingSpeaker, MeetingStatus};

    fn segment(
        id: i64,
        source: Source,
        speaker: Option<u32>,
        start_ms: i64,
        text: &str,
    ) -> MeetingSegment {
        MeetingSegment {
            id,
            meeting_id: 1,
            source,
            speaker,
            start_ms,
            end_ms: start_ms + 1_000,
            text: text.into(),
        }
    }

    #[test]
    fn transcript_uses_names_and_merges_turns() {
        let detail = MeetingDetail {
            meeting: Meeting {
                id: 1,
                title: "Sync".into(),
                started_at: 0,
                ended_at: None,
                status: MeetingStatus::Done,
                system_audio: true,
                speakers_identified: true,
                app: None,
            },
            segments: vec![
                segment(3, Source::System, Some(2), 70_000, "Agreed."),
                segment(1, Source::Mic, None, 1_000, "Hi."),
                segment(2, Source::Mic, None, 3_000, "Ship Friday?"),
                segment(4, Source::System, Some(1), 75_000, "Fine."),
            ],
            speakers: vec![
                MeetingSpeaker {
                    speaker: 1,
                    name: Some("Dana".into()),
                },
                MeetingSpeaker {
                    speaker: 2,
                    name: None,
                },
            ],
            notes: None,
            identifying_speakers: false,
            writing_notes: false,
        };
        assert_eq!(
            transcript_lines(&detail),
            [
                "[0:01] Me: Hi. Ship Friday?",
                "[1:10] Speaker 2: Agreed.",
                "[1:15] Dana: Fine.",
            ]
        );
    }

    #[test]
    fn pack_keeps_lines_whole_and_cuts_long_ones_at_words() {
        let lines = ["aaa bbb".to_string(), "cc".into(), "ddd eee fff".into()];
        assert_eq!(pack(&lines, 10), ["aaa bbb\ncc", "ddd eee", "fff"]);
        assert_eq!(pack(&lines, 100), ["aaa bbb\ncc\nddd eee fff"]);
        assert!(pack(&[], 10).is_empty());
    }
}
