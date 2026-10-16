//! Cuts one channel of a meeting into utterances from per-frame voice
//! decisions, so each can be transcribed while the meeting goes on.
//!
//! Platform- and model-independent: the caller runs the VAD and passes its
//! verdict with every frame.

use std::collections::VecDeque;

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;

const SAMPLE_RATE: usize = WHISPER_SAMPLE_RATE as usize;
/// Audio kept from before the first voiced frame, so soft word onsets survive.
const PREROLL_MS: usize = 300;
/// Voiced frames in a row that open an utterance; shorter blips are noise.
const ONSET_MS: usize = 64;
/// Silence that ends an utterance.
const END_SILENCE_MS: usize = 700;
/// Past this length an utterance ends at the next short pause...
const SOFT_MAX_MS: usize = 15_000;
const SOFT_MAX_PAUSE_MS: usize = 200;
/// ...and is cut here even without one, keeping chunks within what the
/// models transcribe in one pass.
const HARD_MAX_MS: usize = 25_000;
/// Shorter utterances (coughs, clicks) are dropped.
const MIN_SPEECH_MS: usize = 300;
/// Silence left on the end of an utterance.
const TAIL_MS: usize = 200;

/// A stretch of speech, positioned in the channel's timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    /// Offset of the first sample from the start of the channel.
    pub start: usize,
    pub samples: Vec<f32>,
}

impl Utterance {
    pub fn start_ms(&self) -> i64 {
        samples_to_ms(self.start)
    }

    pub fn end_ms(&self) -> i64 {
        samples_to_ms(self.start + self.samples.len())
    }
}

pub fn samples_to_ms(samples: usize) -> i64 {
    (samples as u64 * 1000 / SAMPLE_RATE as u64) as i64
}

struct Open {
    start: usize,
    samples: Vec<f32>,
    /// Voiced samples, to drop utterances that are mostly silence.
    voiced: usize,
    /// Unvoiced frames since the last voiced one.
    silent_frames: usize,
}

pub struct Segmenter {
    frame_samples: usize,
    /// Samples pushed so far: the position of the next frame.
    position: usize,
    preroll: VecDeque<Vec<f32>>,
    /// Voiced frames in a row while no utterance is open.
    onset: usize,
    open: Option<Open>,
}

impl Segmenter {
    pub fn new(frame_samples: usize) -> Self {
        Self {
            frame_samples,
            position: 0,
            preroll: VecDeque::new(),
            onset: 0,
            open: None,
        }
    }

    fn frames(&self, ms: usize) -> usize {
        frames_for(ms, self.frame_samples)
    }

    /// Feeds one frame and its voice decision, returning an utterance when
    /// this frame ended one.
    pub fn push(&mut self, frame: &[f32], voiced: bool) -> Option<Utterance> {
        let frame_start = self.position;
        self.position += frame.len();

        let Some(open) = self.open.as_mut() else {
            self.onset = if voiced { self.onset + 1 } else { 0 };
            self.preroll.push_back(frame.to_vec());
            let keep = self.frames(PREROLL_MS) + self.onset;
            while self.preroll.len() > keep {
                self.preroll.pop_front();
            }
            if self.onset >= self.frames(ONSET_MS) {
                let start =
                    frame_start + frame.len() - self.preroll.iter().map(Vec::len).sum::<usize>();
                let samples: Vec<f32> = self.preroll.drain(..).flatten().collect();
                self.open = Some(Open {
                    start,
                    voiced: self.onset * self.frame_samples,
                    samples,
                    silent_frames: 0,
                });
                self.onset = 0;
            }
            return None;
        };

        open.samples.extend_from_slice(frame);
        if voiced {
            open.voiced += frame.len();
            open.silent_frames = 0;
        } else {
            open.silent_frames += 1;
        }

        let length = open.samples.len();
        let ends = open.silent_frames >= frames_for(END_SILENCE_MS, self.frame_samples)
            || (length >= ms_to_samples(SOFT_MAX_MS)
                && open.silent_frames >= frames_for(SOFT_MAX_PAUSE_MS, self.frame_samples))
            || length >= ms_to_samples(HARD_MAX_MS);
        if ends {
            self.close()
        } else {
            None
        }
    }

    /// Ends the utterance in progress, if any (the recording stopped).
    pub fn finish(&mut self) -> Option<Utterance> {
        self.close()
    }

    fn close(&mut self) -> Option<Utterance> {
        let mut open = self.open.take()?;
        let trailing_silence = open.silent_frames * self.frame_samples;
        let keep = open.samples.len() - trailing_silence.saturating_sub(ms_to_samples(TAIL_MS));
        open.samples.truncate(keep);
        (open.voiced >= ms_to_samples(MIN_SPEECH_MS)).then_some(Utterance {
            start: open.start,
            samples: open.samples,
        })
    }
}

/// Whole frames covering `ms`, rounded up.
fn frames_for(ms: usize, frame_samples: usize) -> usize {
    (ms * SAMPLE_RATE).div_ceil(1000 * frame_samples)
}

fn ms_to_samples(ms: usize) -> usize {
    ms * SAMPLE_RATE / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: usize = 512;

    /// Feeds `pattern` (one bool per frame) and collects the utterances.
    fn run(pattern: &[bool]) -> Vec<Utterance> {
        let mut segmenter = Segmenter::new(FRAME);
        let mut out = Vec::new();
        for (i, &voiced) in pattern.iter().enumerate() {
            let frame = vec![i as f32; FRAME];
            out.extend(segmenter.push(&frame, voiced));
        }
        out.extend(segmenter.finish());
        out
    }

    fn frames(ms: usize) -> usize {
        frames_for(ms, FRAME)
    }

    #[test]
    fn speech_between_silences_is_one_utterance_with_preroll() {
        let mut pattern = vec![false; 50];
        pattern.extend(vec![true; 40]);
        pattern.extend(vec![false; 60]);
        let out = run(&pattern);
        assert_eq!(out.len(), 1);
        let preroll = frames(PREROLL_MS);
        // Starts `preroll` frames before the first voiced frame (index 50).
        assert_eq!(out[0].start, (50 - preroll) * FRAME);
        assert_eq!(out[0].samples[0], (50 - preroll) as f32);
        // Ends with at most TAIL_MS of silence after the last voiced frame.
        let end = out[0].start + out[0].samples.len();
        assert!(end >= 90 * FRAME && end <= 90 * FRAME + ms_to_samples(TAIL_MS) + FRAME);
    }

    #[test]
    fn short_pauses_stay_in_one_utterance() {
        let mut pattern = vec![true; 20];
        pattern.extend(vec![false; frames(END_SILENCE_MS) - 2]);
        pattern.extend(vec![true; 20]);
        pattern.extend(vec![false; 40]);
        assert_eq!(run(&pattern).len(), 1);
    }

    #[test]
    fn long_pauses_split_utterances() {
        let mut pattern = vec![true; 20];
        pattern.extend(vec![false; frames(END_SILENCE_MS) + 1]);
        pattern.extend(vec![true; 20]);
        pattern.extend(vec![false; 40]);
        let out = run(&pattern);
        assert_eq!(out.len(), 2);
        assert!(out[1].start > out[0].start + out[0].samples.len());
    }

    #[test]
    fn blips_and_short_noises_are_dropped() {
        let mut pattern = vec![false; 10];
        pattern.push(true);
        pattern.extend(vec![false; 10]);
        pattern.extend(vec![true; frames(MIN_SPEECH_MS) - 2]);
        pattern.extend(vec![false; 40]);
        assert!(run(&pattern).is_empty());
    }

    #[test]
    fn continuous_speech_is_cut_at_the_hard_limit() {
        let pattern = vec![true; frames(HARD_MAX_MS) * 2 + 10];
        let out = run(&pattern);
        assert!(out.len() >= 2);
        for utterance in &out {
            assert!(utterance.samples.len() <= ms_to_samples(HARD_MAX_MS) + FRAME);
        }
        // No audio is lost between consecutive cuts.
        assert_eq!(out[1].start, out[0].start + out[0].samples.len());
    }

    #[test]
    fn speech_at_the_end_is_flushed_by_finish() {
        let mut pattern = vec![false; 5];
        pattern.extend(vec![true; 30]);
        let out = run(&pattern);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].end_ms(), samples_to_ms(35 * FRAME));
    }
}
