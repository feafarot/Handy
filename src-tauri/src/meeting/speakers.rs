//! Telling the other participants apart once a meeting is over: the system
//! channel is diarized with speakrs (pyannote community-1), every system
//! segment gets the speaker heard most in it, and a segment in which the
//! speaker changes is split at the change and its parts transcribed again.
//!
//! The diarization models (~35 MB) are downloaded on first use. Windows runs
//! them on the GPU through DirectML, falling back to ONNX Runtime's CPU
//! provider like other platforms.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use log::info;
use speakrs::models::{required_model_files, MODEL_REPOSITORY, MODEL_REVISION};
use speakrs::{ExecutionMode, OwnedDiarizationPipeline};
use tauri::AppHandle;

use super::store::{MeetingSegment, MeetingStore, SpeakerAssignment, SplitSegment};
use super::{transcribe, Source, SAMPLE_RATE};

#[cfg(target_os = "windows")]
const MODE: ExecutionMode = ExecutionMode::DirectMl;
#[cfg(not(target_os = "windows"))]
const MODE: ExecutionMode = ExecutionMode::OrtCpu;

/// Resolution of the per-segment speaker timeline.
const BIN_MS: i64 = 100;
/// A speaker run shorter than this inside a segment is noise in the
/// diarization, not a turn of its own.
const MIN_PART_MS: i64 = 1_000;
/// A segment no diarized turn overlaps takes the nearest turn's speaker if
/// that turn is this close.
const NEAREST_TURN_MS: i64 = 2_000;

/// One model runs at a time, however many meetings wait.
static RUNNING: Mutex<()> = Mutex::new(());

/// What identification is doing, for progress in the UI.
pub enum Step {
    DownloadingModels,
    Diarizing,
}

/// Identifies the speakers of a finished meeting and stores the result.
pub fn identify(
    app: &AppHandle,
    store: &MeetingStore,
    meeting_id: i64,
    models_root: &Path,
    on_step: impl Fn(Step),
) -> Result<()> {
    let _running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    let segments = store.system_segments(meeting_id)?;
    if segments.is_empty() {
        store.set_speakers(meeting_id, 0, &[])?;
        return Ok(());
    }
    let dir = models_dir(models_root);
    if !models_present(&dir) {
        on_step(Step::DownloadingModels);
        download_models(&dir)?;
    }
    on_step(Step::Diarizing);
    let audio = read_wav(&store.audio_dir(meeting_id).join(Source::System.file_name()))?;
    let turns = diarize(&dir, &audio)?;

    let spans: Vec<Span> = segments.iter().map(Span::of).collect();
    let mut assignments = Vec::new();
    for (segment, span) in segments.iter().zip(&spans) {
        match assign(span, &turns) {
            Assignment::Unknown => {}
            Assignment::Speaker(speaker) => assignments.push(Pending::Label(segment.id, speaker)),
            Assignment::Split(parts) => {
                let mut transcribed = Vec::new();
                for part in &parts {
                    let samples = slice(&audio, part.start_ms, part.end_ms);
                    if let Some(text) = transcribe(app, samples) {
                        let text = text.trim();
                        if !text.is_empty() {
                            transcribed.push((part.clone(), text.to_string()));
                        }
                    }
                }
                if transcribed.is_empty() {
                    // Nothing intelligible on its own: keep the segment whole.
                    assignments.push(Pending::Label(segment.id, dominant(&parts)));
                } else {
                    assignments.push(Pending::Split(segment.id, transcribed));
                }
            }
        }
    }
    let (speakers, assignments) = number_speakers(assignments);
    store.set_speakers(meeting_id, speakers, &assignments)?;
    info!("Meeting {meeting_id}: {speakers} other speaker(s) identified");
    Ok(())
}

fn models_dir(models_root: &Path) -> PathBuf {
    models_root
        .join("speaker-diarization")
        .join(&MODEL_REVISION[..12])
}

fn models_present(dir: &Path) -> bool {
    required_model_files(MODE)
        .iter()
        .all(|file| dir.join(file).is_file())
}

fn download_models(dir: &Path) -> Result<()> {
    let started = Instant::now();
    tauri::async_runtime::block_on(async {
        let client = reqwest::Client::new();
        for file in required_model_files(MODE) {
            let path = dir.join(&file);
            if path.is_file() {
                continue;
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let url = format!(
                "https://huggingface.co/{MODEL_REPOSITORY}/resolve/{MODEL_REVISION}/{file}"
            );
            let response = client
                .get(&url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .with_context(|| format!("downloading {file} failed"))?;
            // Written aside and renamed, so a cut download is never mistaken
            // for a complete one.
            let partial = PathBuf::from(format!("{}.part", path.display()));
            let mut output = std::fs::File::create(&partial)?;
            let mut body = response.bytes_stream();
            while let Some(chunk) = body.next().await {
                output.write_all(&chunk.with_context(|| format!("downloading {file} failed"))?)?;
            }
            output.flush()?;
            drop(output);
            std::fs::rename(&partial, &path)?;
        }
        anyhow::Ok(())
    })?;
    info!(
        "Downloaded the speaker diarization models in {:.1?}",
        started.elapsed()
    );
    Ok(())
}

fn read_wav(path: &Path) -> Result<Vec<f32>> {
    let mut reader =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    reader
        .samples::<i16>()
        .map(|sample| Ok(f32::from(sample?) / 32768.0))
        .collect()
}

fn slice(audio: &[f32], start_ms: i64, end_ms: i64) -> &[f32] {
    let sample = |ms: i64| (ms.max(0) as usize * SAMPLE_RATE as usize / 1000).min(audio.len());
    let start = sample(start_ms);
    &audio[start..sample(end_ms).max(start)]
}

/// A stretch in which one diarized speaker talks.
#[derive(Clone, Debug, PartialEq)]
struct Turn {
    start_ms: i64,
    end_ms: i64,
    speaker: usize,
}

fn diarize(models: &Path, audio: &[f32]) -> Result<Vec<Turn>> {
    let started = Instant::now();
    let mut pipeline = OwnedDiarizationPipeline::from_dir(models, MODE)
        .map_err(|e| anyhow!("loading the diarization models failed: {e}"))?;
    let result = pipeline
        .run(audio)
        .map_err(|e| anyhow!("diarization failed: {e}"))?;
    let mut labels = HashMap::new();
    let mut turns: Vec<Turn> = result
        .discrete_diarization
        .to_segments()
        .into_iter()
        .map(|segment| {
            let next = labels.len();
            Turn {
                start_ms: (segment.start * 1000.0) as i64,
                end_ms: (segment.end * 1000.0) as i64,
                speaker: *labels.entry(segment.speaker).or_insert(next),
            }
        })
        .collect();
    turns.sort_by_key(|turn| turn.start_ms);
    info!(
        "Diarized {:.0}s of meeting audio in {:.1?}: {} turn(s), {} speaker(s)",
        audio.len() as f64 / 16_000.0,
        started.elapsed(),
        turns.len(),
        labels.len()
    );
    Ok(turns)
}

#[derive(Clone, Copy)]
struct Span {
    start_ms: i64,
    end_ms: i64,
}

impl Span {
    fn of(segment: &MeetingSegment) -> Self {
        Self {
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
        }
    }
}

#[derive(Debug, PartialEq)]
enum Assignment {
    /// No diarized speech near the segment.
    Unknown,
    Speaker(usize),
    /// Consecutive parts covering the segment, each one speaker's.
    Split(Vec<Turn>),
}

/// Who speaks in a segment: per bin, the speaker overlapping it most, with
/// runs too short to be a turn absorbed into their neighbours.
fn assign(span: &Span, turns: &[Turn]) -> Assignment {
    let bins = ((span.end_ms - span.start_ms + BIN_MS - 1) / BIN_MS).max(1) as usize;
    let mut speakers: Vec<Option<usize>> = (0..bins)
        .map(|bin| {
            let start = span.start_ms + bin as i64 * BIN_MS;
            let end = (start + BIN_MS).min(span.end_ms);
            let mut best = None;
            let mut best_overlap = 0;
            for turn in turns {
                let overlap = turn.end_ms.min(end) - turn.start_ms.max(start);
                if overlap > best_overlap {
                    best_overlap = overlap;
                    best = Some(turn.speaker);
                }
            }
            best
        })
        .collect();

    if speakers.iter().all(Option::is_none) {
        return turns
            .iter()
            .map(|turn| {
                let gap = (turn.start_ms - span.end_ms).max(span.start_ms - turn.end_ms);
                (gap, turn.speaker)
            })
            .filter(|&(gap, _)| gap <= NEAREST_TURN_MS)
            .min_by_key(|&(gap, _)| gap)
            .map_or(Assignment::Unknown, |(_, speaker)| {
                Assignment::Speaker(speaker)
            });
    }
    fill_gaps(&mut speakers);

    let mut runs = runs_of(&speakers);
    let min_bins = (MIN_PART_MS / BIN_MS) as usize;
    while runs.len() > 1 {
        let Some((shortest, _)) = runs
            .iter()
            .enumerate()
            .filter(|(_, run)| run.len < min_bins)
            .min_by_key(|(_, run)| run.len)
        else {
            break;
        };
        // Merge into the longer neighbour.
        let into = match (shortest.checked_sub(1), runs.get(shortest + 1)) {
            (Some(before), Some(after)) if after.len > runs[before].len => shortest + 1,
            (Some(before), _) => before,
            (None, _) => shortest + 1,
        };
        let len = runs[shortest].len;
        runs[into].len += len;
        if into < shortest {
            runs.remove(shortest);
        } else {
            runs[into].start -= len;
            runs.remove(shortest);
        }
        // Neighbours of the same speaker now touch.
        let mut merged: Vec<Run> = Vec::with_capacity(runs.len());
        for run in runs {
            match merged.last_mut() {
                Some(last) if last.speaker == run.speaker => last.len += run.len,
                _ => merged.push(run),
            }
        }
        runs = merged;
    }

    if runs.len() == 1 {
        return Assignment::Speaker(runs[0].speaker);
    }
    let parts = runs
        .iter()
        .map(|run| Turn {
            start_ms: span.start_ms + run.start as i64 * BIN_MS,
            end_ms: (span.start_ms + (run.start + run.len) as i64 * BIN_MS).min(span.end_ms),
            speaker: run.speaker,
        })
        .collect();
    Assignment::Split(parts)
}

/// Splits each silent stretch between the speakers on either side, so a cut
/// lands in the middle of the pause rather than on a word.
fn fill_gaps(speakers: &mut [Option<usize>]) {
    let mut index = 0;
    while index < speakers.len() {
        if speakers[index].is_some() {
            index += 1;
            continue;
        }
        let end = speakers[index..]
            .iter()
            .position(Option::is_some)
            .map_or(speakers.len(), |offset| index + offset);
        let before = index.checked_sub(1).and_then(|i| speakers[i]);
        let after = speakers.get(end).copied().flatten();
        let middle = index + (end - index) / 2;
        for (bin, speaker) in speakers[index..end].iter_mut().enumerate() {
            *speaker = if index + bin < middle {
                before.or(after)
            } else {
                after.or(before)
            };
        }
        index = end;
    }
}

struct Run {
    speaker: usize,
    start: usize,
    len: usize,
}

fn runs_of(speakers: &[Option<usize>]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (index, speaker) in speakers.iter().enumerate() {
        let speaker = speaker.unwrap_or_default();
        match runs.last_mut() {
            Some(last) if last.speaker == speaker => last.len += 1,
            _ => runs.push(Run {
                speaker,
                start: index,
                len: 1,
            }),
        }
    }
    runs
}

fn dominant(parts: &[Turn]) -> usize {
    let mut totals: HashMap<usize, i64> = HashMap::new();
    for part in parts {
        *totals.entry(part.speaker).or_default() += part.end_ms - part.start_ms;
    }
    totals
        .into_iter()
        .max_by_key(|&(speaker, total)| (total, std::cmp::Reverse(speaker)))
        .map_or(0, |(speaker, _)| speaker)
}

/// An assignment still in diarization's speaker indices.
enum Pending {
    Label(i64, usize),
    Split(i64, Vec<(Turn, String)>),
}

/// Numbers the speakers 1, 2, ... in the order they first speak, dropping
/// any diarization found but no segment ended up with.
fn number_speakers(pending: Vec<Pending>) -> (u32, Vec<SpeakerAssignment>) {
    let mut numbers: HashMap<usize, u32> = HashMap::new();
    let mut number = |speaker: usize| {
        let next = numbers.len() as u32 + 1;
        *numbers.entry(speaker).or_insert(next)
    };
    let assignments = pending
        .into_iter()
        .map(|pending| match pending {
            Pending::Label(segment_id, speaker) => SpeakerAssignment::Label {
                segment_id,
                speaker: number(speaker),
            },
            Pending::Split(segment_id, parts) => SpeakerAssignment::Split {
                segment_id,
                parts: parts
                    .into_iter()
                    .map(|(turn, text)| SplitSegment {
                        speaker: number(turn.speaker),
                        start_ms: turn.start_ms,
                        end_ms: turn.end_ms,
                        text,
                    })
                    .collect(),
            },
        })
        .collect();
    (numbers.len() as u32, assignments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(start_ms: i64, end_ms: i64, speaker: usize) -> Turn {
        Turn {
            start_ms,
            end_ms,
            speaker,
        }
    }

    fn span(start_ms: i64, end_ms: i64) -> Span {
        Span { start_ms, end_ms }
    }

    #[test]
    fn a_segment_inside_one_turn_gets_its_speaker() {
        let turns = [turn(0, 5_000, 0), turn(5_500, 9_000, 1)];
        assert_eq!(assign(&span(6_000, 8_000), &turns), Assignment::Speaker(1));
    }

    #[test]
    fn a_short_interjection_does_not_split_a_segment() {
        let turns = [
            turn(0, 3_000, 0),
            turn(3_000, 3_400, 1),
            turn(3_400, 6_000, 0),
        ];
        assert_eq!(assign(&span(0, 6_000), &turns), Assignment::Speaker(0));
    }

    #[test]
    fn a_speaker_change_splits_the_segment_at_the_change() {
        let turns = [turn(0, 2_950, 0), turn(3_200, 6_000, 1)];
        assert_eq!(
            assign(&span(0, 6_000), &turns),
            Assignment::Split(vec![turn(0, 3_100, 0), turn(3_100, 6_000, 1)])
        );
    }

    #[test]
    fn segments_without_turns_take_a_nearby_speaker_or_none() {
        let turns = [turn(0, 2_000, 0), turn(10_000, 12_000, 1)];
        assert_eq!(assign(&span(3_000, 3_500), &turns), Assignment::Speaker(0));
        assert_eq!(assign(&span(5_000, 5_500), &turns), Assignment::Unknown);
    }

    #[test]
    fn speakers_are_numbered_by_first_appearance() {
        let (count, assignments) = number_speakers(vec![
            Pending::Label(1, 3),
            Pending::Split(
                2,
                vec![
                    (turn(0, 1_000, 3), "a".into()),
                    (turn(1_000, 2_000, 0), "b".into()),
                ],
            ),
        ]);
        assert_eq!(count, 2);
        let numbers: Vec<u32> = assignments
            .iter()
            .flat_map(|assignment| match assignment {
                SpeakerAssignment::Label { speaker, .. } => vec![*speaker],
                SpeakerAssignment::Split { parts, .. } => {
                    parts.iter().map(|part| part.speaker).collect()
                }
            })
            .collect();
        assert_eq!(numbers, [1, 1, 2]);
    }
}
