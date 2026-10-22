//! Meeting recording: the microphone (the user) and, where the platform
//! supports it, system audio (the other participants) are recorded as two
//! channels, cut into utterances by VAD and transcribed while the meeting
//! goes on, with the selected transcription model.
//!
//! Each channel runs on its own thread: it writes the channel's WAV file,
//! runs the VAD and hands finished utterances to one transcriber thread, which
//! also serves dictation's model, so dictation keeps working during a meeting.
//! Everything lives here rather than in the dictation pipeline, which it only
//! touches through `TranscriptionManager::transcribe`.
//!
//! Once a meeting ends, the other participants are told apart (see
//! `speakers`), named from the conversation (see `names`) and notes are
//! written (see `notes`), in the background.
//!
//! Meetings can also start (and stop) on their own when an app starts using
//! the microphone; see `auto`.

pub mod auto;
mod claude;
mod names;
mod notes;
mod segmenter;
mod speakers;
mod store;
mod system_audio;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use handy_recorder::{AudioChunk, Channels, Recorder, RecorderConfig, Sink};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::audio_toolkit::{SileroVad, VoiceActivityDetector};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{get_settings, MeetingNotesWriter};
use segmenter::{samples_to_ms, Segmenter, Utterance};
use store::MeetingStore;
pub use store::{Meeting, MeetingDetail, MeetingSegment, Source};

const VAD_MODEL: &str = "resources/models/silero-vad-v6.2-F32.gguf";
const VAD_THRESHOLD: f32 = 0.3;
const SAMPLE_RATE: u32 = crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
/// How often the WAV headers are rewritten, bounding what a crash loses.
const WAV_FLUSH_INTERVAL: Duration = Duration::from_secs(5);
/// A mic utterance this quiet that mostly overlaps the other side talking is
/// their voice leaking from speakers into the microphone, not the user.
const BLEED_MAX_RMS: f32 = 0.02;
const BLEED_MIN_OVERLAP: f32 = 0.6;
const TRANSCRIBE_ATTEMPTS: usize = 3;
/// Whether a platform can record system audio, for the UI.
pub const SYSTEM_AUDIO_SUPPORTED: bool = system_audio::SUPPORTED;

/// The installed Claude Code's version, for the settings page.
pub fn claude_code_version() -> Result<String> {
    claude::version()
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "phase", rename_all = "lowercase")]
pub enum MeetingState {
    Idle,
    Recording {
        meeting: Meeting,
    },
    /// Recording stopped; the last utterances are still being transcribed.
    Finishing {
        meeting_id: i64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingStateEvent {
    pub state: MeetingState,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingSegmentEvent {
    pub segment: MeetingSegment,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "step", rename_all = "lowercase")]
pub enum SpeakersProgress {
    /// First use: fetching the diarization models.
    Downloading,
    Identifying,
    Done,
    Failed {
        error: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingSpeakersEvent {
    pub meeting_id: i64,
    pub progress: SpeakersProgress,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "step", rename_all = "lowercase")]
pub enum NotesProgress {
    /// Working out the unnamed speakers' names from the conversation.
    Naming,
    /// `done` of `total` requests to the LLM have finished.
    Writing {
        done: u32,
        total: u32,
    },
    Done,
    Failed {
        error: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MeetingNotesEvent {
    pub meeting_id: i64,
    pub progress: NotesProgress,
}

/// Work done on a finished meeting in the background.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Task {
    Speakers,
    Notes,
}

enum State {
    Idle,
    Recording(Box<Session>),
    Finishing(i64),
}

pub struct MeetingManager {
    app: AppHandle,
    store: Arc<MeetingStore>,
    state: Mutex<State>,
    models_dir: PathBuf,
    /// Background tasks running, per meeting.
    tasks: Mutex<HashSet<(i64, Task)>>,
}

impl MeetingManager {
    pub fn new(app: &AppHandle) -> Result<Self> {
        let app_data_dir = crate::portable::app_data_dir(app)?;
        Ok(Self {
            app: app.clone(),
            store: Arc::new(MeetingStore::open(&app_data_dir)?),
            state: Mutex::new(State::Idle),
            models_dir: app_data_dir.join("models"),
            tasks: Mutex::new(HashSet::new()),
        })
    }

    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn state(&self) -> MeetingState {
        match &*self.lock_state() {
            State::Idle => MeetingState::Idle,
            State::Recording(session) => MeetingState::Recording {
                meeting: session.meeting.clone(),
            },
            State::Finishing(meeting_id) => MeetingState::Finishing {
                meeting_id: *meeting_id,
            },
        }
    }

    fn emit_state(&self) {
        let event = MeetingStateEvent {
            state: self.state(),
        };
        if let Err(e) = event.emit(&self.app) {
            warn!("Failed to emit meeting state: {e}");
        }
    }

    /// Starts recording. `app` is what the meeting is held in, if known.
    pub fn start(&self, title: &str, system_audio: bool, app: Option<&str>) -> Result<Meeting> {
        let mut state = self.lock_state();
        match &*state {
            State::Idle => {}
            State::Recording(_) => return Err(anyhow!("a meeting is already being recorded")),
            State::Finishing(_) => {
                return Err(anyhow!("the previous meeting is still being transcribed"))
            }
        }
        let session = Session::start(&self.app, &self.store, title, system_audio, app)?;
        let meeting = session.meeting.clone();
        *state = State::Recording(Box::new(session));
        drop(state);
        info!(
            "Meeting {} started (system audio: {}, app: {})",
            meeting.id,
            meeting.system_audio,
            meeting.app.as_deref().unwrap_or("unknown")
        );
        self.emit_state();
        Ok(meeting)
    }

    /// Stops recording. Returns at once; the remaining utterances are
    /// transcribed in the background and the state returns to idle after.
    pub fn stop(self: &Arc<Self>) -> Result<()> {
        let session = {
            let mut state = self.lock_state();
            let State::Recording(session) = std::mem::replace(&mut *state, State::Idle) else {
                return Err(anyhow!("no meeting is being recorded"));
            };
            *state = State::Finishing(session.meeting.id);
            session
        };
        self.emit_state();
        let manager = Arc::clone(self);
        std::thread::spawn(move || {
            let meeting_id = session.meeting.id;
            let system_audio = session.meeting.system_audio;
            let ended_at = session.finish();
            if let Err(e) = manager.store.finish(meeting_id, ended_at) {
                error!("Failed to close meeting {meeting_id}: {e}");
            }
            info!("Meeting {meeting_id} finished");
            *manager.lock_state() = State::Idle;
            manager.emit_state();
            // Speakers first, so the notes can tell them apart.
            if system_audio {
                manager.run_speakers(meeting_id);
            }
            let settings = get_settings(&manager.app);
            if settings.meeting_auto_notes {
                let ready = manager
                    .finished_meeting(meeting_id)
                    .and_then(|meeting| notes::Llm::for_meeting(&settings, &meeting));
                match ready {
                    Ok(_) => manager.run_notes(meeting_id),
                    Err(e) => info!("Not writing notes for meeting {meeting_id}: {e}"),
                }
            }
        });
        Ok(())
    }

    /// Tells the other participants of a finished meeting apart, in the
    /// background; progress arrives as `MeetingSpeakersEvent`s. Running it
    /// again starts over, dropping the speaker names given so far.
    pub fn identify_speakers(self: &Arc<Self>, meeting_id: i64) -> Result<()> {
        let meeting = self.finished_meeting(meeting_id)?;
        if !meeting.system_audio {
            return Err(anyhow!("the meeting has no system audio"));
        }
        self.spawn("meeting-speakers", move |manager| {
            manager.run_speakers(meeting_id)
        })
    }

    /// Writes a finished meeting's notes in the background, replacing any
    /// written before; progress arrives as `MeetingNotesEvent`s.
    pub fn write_notes(self: &Arc<Self>, meeting_id: i64) -> Result<()> {
        let meeting = self.finished_meeting(meeting_id)?;
        notes::Llm::for_meeting(&get_settings(&self.app), &meeting)?;
        self.spawn("meeting-notes", move |manager| {
            manager.run_notes(meeting_id)
        })
    }

    fn finished_meeting(&self, meeting_id: i64) -> Result<Meeting> {
        if self.is_active(meeting_id) {
            return Err(anyhow!("the meeting is still being recorded"));
        }
        Ok(self
            .store
            .get(meeting_id)?
            .ok_or_else(|| anyhow!("no meeting {meeting_id}"))?
            .meeting)
    }

    fn spawn(
        self: &Arc<Self>,
        name: &str,
        task: impl FnOnce(&MeetingManager) + Send + 'static,
    ) -> Result<()> {
        let manager = Arc::clone(self);
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || task(&manager))?;
        Ok(())
    }

    /// Marks a task as running; false if it already was.
    fn claim(&self, meeting_id: i64, task: Task) -> bool {
        self.lock_tasks().insert((meeting_id, task))
    }

    fn release(&self, meeting_id: i64, task: Task) {
        self.lock_tasks().remove(&(meeting_id, task));
    }

    fn is_running(&self, meeting_id: i64, task: Task) -> bool {
        self.lock_tasks().contains(&(meeting_id, task))
    }

    fn lock_tasks(&self) -> MutexGuard<'_, HashSet<(i64, Task)>> {
        self.tasks.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run_speakers(&self, meeting_id: i64) {
        if !self.claim(meeting_id, Task::Speakers) {
            return;
        }
        self.emit_speakers(meeting_id, SpeakersProgress::Identifying);
        let result = speakers::identify(
            &self.app,
            &self.store,
            meeting_id,
            &self.models_dir,
            |step| {
                self.emit_speakers(
                    meeting_id,
                    match step {
                        speakers::Step::DownloadingModels => SpeakersProgress::Downloading,
                        speakers::Step::Diarizing => SpeakersProgress::Identifying,
                    },
                )
            },
        );
        self.release(meeting_id, Task::Speakers);
        let progress = match result {
            Ok(()) => SpeakersProgress::Done,
            Err(e) => {
                error!("Identifying the speakers of meeting {meeting_id} failed: {e:#}");
                SpeakersProgress::Failed {
                    error: format!("{e:#}"),
                }
            }
        };
        self.emit_speakers(meeting_id, progress);
    }

    fn run_notes(&self, meeting_id: i64) {
        if !self.claim(meeting_id, Task::Notes) {
            return;
        }
        let result = notes::write(&self.app, &self.store, meeting_id, |step| match step {
            notes::Step::NamingSpeakers => self.emit_notes(meeting_id, NotesProgress::Naming),
            notes::Step::SpeakersNamed => self.emit_speakers(meeting_id, SpeakersProgress::Done),
            notes::Step::Writing { done, total } => {
                self.emit_notes(meeting_id, NotesProgress::Writing { done, total })
            }
        });
        self.release(meeting_id, Task::Notes);
        let progress = match result {
            Ok(()) => NotesProgress::Done,
            Err(e) => {
                error!("Writing the notes of meeting {meeting_id} failed: {e:#}");
                NotesProgress::Failed {
                    error: format!("{e:#}"),
                }
            }
        };
        self.emit_notes(meeting_id, progress);
    }

    fn emit_notes(&self, meeting_id: i64, progress: NotesProgress) {
        if let Err(e) = (MeetingNotesEvent {
            meeting_id,
            progress,
        })
        .emit(&self.app)
        {
            warn!("Failed to emit notes progress: {e}");
        }
    }

    fn emit_speakers(&self, meeting_id: i64, progress: SpeakersProgress) {
        if let Err(e) = (MeetingSpeakersEvent {
            meeting_id,
            progress,
        })
        .emit(&self.app)
        {
            warn!("Failed to emit speaker progress: {e}");
        }
    }

    pub fn rename_speaker(&self, meeting_id: i64, speaker: u32, name: Option<&str>) -> Result<()> {
        let name = name.map(str::trim).filter(|name| !name.is_empty());
        self.store.rename_speaker(meeting_id, speaker, name)
    }

    /// Folds one speaker into another, for when identification told one
    /// person apart as two. Not while identification runs: its result would
    /// replace the speakers anyway.
    pub fn merge_speakers(&self, meeting_id: i64, from: u32, into: u32) -> Result<()> {
        if self.is_running(meeting_id, Task::Speakers) {
            return Err(anyhow!("the speakers are still being identified"));
        }
        self.store.merge_speakers(meeting_id, from, into)
    }

    pub fn list(&self) -> Result<Vec<Meeting>> {
        self.store.list()
    }

    pub fn get(&self, meeting_id: i64) -> Result<Option<MeetingDetail>> {
        let mut detail = self.store.get(meeting_id)?;
        if let Some(detail) = &mut detail {
            detail.identifying_speakers = self.is_running(meeting_id, Task::Speakers);
            detail.writing_notes = self.is_running(meeting_id, Task::Notes);
        }
        Ok(detail)
    }

    pub fn rename(&self, meeting_id: i64, title: &str) -> Result<()> {
        self.store.rename(meeting_id, title)
    }

    /// Sets what writes the meeting's notes; `None` follows the setting.
    pub fn set_notes_writer(
        &self,
        meeting_id: i64,
        writer: Option<MeetingNotesWriter>,
    ) -> Result<()> {
        self.store.set_notes_writer(meeting_id, writer)?;
        let mut state = self.lock_state();
        if let State::Recording(session) = &mut *state {
            if session.meeting.id == meeting_id {
                session.meeting.notes_writer = writer;
                drop(state);
                self.emit_state();
            }
        }
        Ok(())
    }

    /// Sets the app the meeting was held in; an empty or missing name means
    /// unknown.
    pub fn set_app(&self, meeting_id: i64, app: Option<&str>) -> Result<()> {
        let app = app.map(str::trim).filter(|app| !app.is_empty());
        self.store.set_app(meeting_id, app)?;
        let mut state = self.lock_state();
        if let State::Recording(session) = &mut *state {
            if session.meeting.id == meeting_id {
                session.meeting.app = app.map(str::to_string);
                drop(state);
                self.emit_state();
            }
        }
        Ok(())
    }

    pub fn delete(&self, meeting_id: i64) -> Result<()> {
        if self.is_active(meeting_id) {
            return Err(anyhow!("the meeting is still being recorded"));
        }
        if self.is_running(meeting_id, Task::Speakers) || self.is_running(meeting_id, Task::Notes) {
            return Err(anyhow!("the meeting is still being processed"));
        }
        self.store.delete(meeting_id)
    }

    pub fn audio_dir(&self, meeting_id: i64) -> PathBuf {
        self.store.audio_dir(meeting_id)
    }

    fn is_active(&self, meeting_id: i64) -> bool {
        match &*self.lock_state() {
            State::Idle => false,
            State::Recording(session) => session.meeting.id == meeting_id,
            State::Finishing(id) => *id == meeting_id,
        }
    }
}

/// A meeting being recorded.
struct Session {
    meeting: Meeting,
    started: Instant,
    microphone: Option<Recorder<MicSink>>,
    system: Option<system_audio::SystemAudioCapture>,
    channels: Vec<JoinHandle<()>>,
    transcriber: JoinHandle<()>,
}

impl Session {
    fn start(
        app: &AppHandle,
        store: &Arc<MeetingStore>,
        title: &str,
        system: bool,
        app_name: Option<&str>,
    ) -> Result<Self> {
        let vad_path = app
            .path()
            .resolve(VAD_MODEL, tauri::path::BaseDirectory::Resource)
            .map_err(|e| anyhow!("failed to resolve the VAD model: {e}"))?;
        let started_at = chrono::Utc::now().timestamp_millis();
        let (jobs_tx, jobs_rx) = mpsc::channel::<Job>();
        let other_side_speech = Arc::new(Mutex::new(SpeechTimeline::default()));

        // System audio first: if it can't start, the meeting is mic-only and
        // is stored as such.
        let (system_tx, system_rx) = mpsc::channel::<Vec<f32>>();
        let system_capture = if system {
            match system_audio::SystemAudioCapture::start(Box::new(move |samples| {
                let _ = system_tx.send(samples.to_vec());
            })) {
                Ok(capture) => Some(capture),
                Err(e) => {
                    warn!("Recording the meeting without system audio: {e}");
                    None
                }
            }
        } else {
            None
        };

        let meeting = store.create(title, started_at, system_capture.is_some(), app_name)?;
        let dir = store.audio_dir(meeting.id);
        let mut channels = Vec::new();
        let start_channel = |source, receiver, timeline| {
            ChannelWorker::spawn(source, receiver, &dir, &vad_path, jobs_tx.clone(), timeline)
        };
        if system_capture.is_some() {
            channels.push(start_channel(
                Source::System,
                system_rx,
                Some(Arc::clone(&other_side_speech)),
            )?);
        }

        let (mic_tx, mic_rx) = mpsc::channel::<Vec<f32>>();
        let microphone = open_microphone(app, MicSink(mic_tx)).inspect_err(|_| {
            let _ = store.delete(meeting.id);
        })?;
        channels.push(start_channel(Source::Mic, mic_rx, None)?);
        drop(jobs_tx);

        let transcriber = Transcriber {
            app: app.clone(),
            store: Arc::clone(store),
            meeting_id: meeting.id,
            other_side_speech,
        };
        let transcriber = std::thread::Builder::new()
            .name("meeting-transcriber".into())
            .spawn(move || transcriber.run(jobs_rx))?;
        app.state::<Arc<TranscriptionManager>>()
            .initiate_model_load();

        Ok(Self {
            meeting,
            started: Instant::now(),
            microphone: Some(microphone),
            system: system_capture,
            channels,
            transcriber,
        })
    }

    /// Stops capture and waits until every utterance is transcribed. Returns
    /// the meeting's end time.
    fn finish(mut self) -> i64 {
        let ended_at = self.stop_capture();
        let _ = self.transcriber.join();
        ended_at
    }

    /// Stops capture and finalizes the audio files, leaving the transcriber
    /// to drain on its own. Returns the meeting's end time.
    fn stop_capture(&mut self) -> i64 {
        let ended_at = self.meeting.started_at + self.started.elapsed().as_millis() as i64;
        if let Some(microphone) = self.microphone.take() {
            match microphone.stop() {
                // Dropping the sink closes its channel, ending the worker.
                Ok(stopped) => drop(stopped.sink),
                Err(e) => warn!("Stopping the meeting microphone failed: {e}"),
            }
            if let Err(e) = microphone.close() {
                warn!("Closing the meeting microphone failed: {e}");
            }
        }
        drop(self.system.take());
        for channel in self.channels.drain(..) {
            let _ = channel.join();
        }
        ended_at
    }
}

struct MicSink(mpsc::Sender<Vec<f32>>);

impl Sink for MicSink {
    fn process_chunk(&mut self, chunk: AudioChunk<'_>) {
        let _ = self.0.send(chunk.samples[..chunk.valid_frames].to_vec());
    }
}

/// Opens the microphone dictation uses, as a second stream of its own.
fn open_microphone(app: &AppHandle, sink: MicSink) -> Result<Recorder<MicSink>> {
    let settings = get_settings(app);
    let device = settings.selected_microphone.as_ref().and_then(|name| {
        handy_recorder::list_input_devices()
            .ok()?
            .into_iter()
            .find(|device| &device.name == name)
            .map(|device| device.id)
    });
    let config = RecorderConfig {
        device,
        channels: settings
            .selected_channel
            .map_or(Channels::MixToMono, Channels::Only),
        ..RecorderConfig::speech()
    };
    let recorder = Recorder::open_with_failure_handler(config, |e| {
        error!("The meeting microphone failed: {e}")
    })
    .map_err(|e| anyhow!("failed to open the microphone: {e}"))?;
    recorder
        .start(sink)
        .map_err(|e| anyhow!("failed to start the microphone: {}", e.error))?;
    Ok(recorder)
}

/// An utterance waiting to be transcribed.
struct Job {
    source: Source,
    utterance: Utterance,
}

/// When the other side was talking, in system-channel samples.
#[derive(Default)]
struct SpeechTimeline(Vec<(usize, usize)>);

impl SpeechTimeline {
    fn add(&mut self, start: usize, end: usize) {
        match self.0.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => self.0.push((start, end)),
        }
    }

    /// Fraction of `start..end` covered.
    fn overlap(&self, start: usize, end: usize) -> f32 {
        if end <= start {
            return 0.0;
        }
        let covered: usize = self
            .0
            .iter()
            .map(|&(s, e)| e.min(end).saturating_sub(s.max(start)))
            .sum();
        covered as f32 / (end - start) as f32
    }
}

/// Writes one channel to disk and cuts it into utterances.
struct ChannelWorker {
    source: Source,
    vad: SileroVad,
    segmenter: Segmenter,
    wav: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    last_flush: Instant,
    pending: Vec<f32>,
    position: usize,
    jobs: mpsc::Sender<Job>,
    /// Set for the system channel, to report when the other side speaks.
    timeline: Option<Arc<Mutex<SpeechTimeline>>>,
    vad_failed: bool,
}

impl ChannelWorker {
    fn spawn(
        source: Source,
        audio: mpsc::Receiver<Vec<f32>>,
        dir: &Path,
        vad_path: &Path,
        jobs: mpsc::Sender<Job>,
        timeline: Option<Arc<Mutex<SpeechTimeline>>>,
    ) -> Result<JoinHandle<()>> {
        let vad = SileroVad::new(vad_path, VAD_THRESHOLD)?;
        let wav = hound::WavWriter::create(
            dir.join(source.file_name()),
            hound::WavSpec {
                channels: 1,
                sample_rate: SAMPLE_RATE,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        let mut worker = Self {
            source,
            segmenter: Segmenter::new(vad.frame_samples()),
            vad,
            wav,
            last_flush: Instant::now(),
            pending: Vec::new(),
            position: 0,
            jobs,
            timeline,
            vad_failed: false,
        };
        Ok(std::thread::Builder::new()
            .name(format!("meeting-{}", source.file_name()))
            .spawn(move || {
                while let Ok(samples) = audio.recv() {
                    worker.process(&samples);
                }
                worker.finish();
            })?)
    }

    fn process(&mut self, samples: &[f32]) {
        for &sample in samples {
            let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
            if let Err(e) = self.wav.write_sample(value) {
                error!("Writing meeting audio failed: {e}");
                break;
            }
        }
        if self.last_flush.elapsed() >= WAV_FLUSH_INTERVAL {
            self.last_flush = Instant::now();
            if let Err(e) = self.wav.flush() {
                warn!("Flushing meeting audio failed: {e}");
            }
        }

        self.pending.extend_from_slice(samples);
        let frame_samples = self.vad.frame_samples();
        let frames = self.pending.len() / frame_samples;
        for index in 0..frames {
            let frame = &self.pending[index * frame_samples..(index + 1) * frame_samples];
            let voiced = match self.vad.is_voice(frame) {
                Ok(voiced) => voiced,
                Err(e) => {
                    if !self.vad_failed {
                        error!("Meeting VAD failed: {e}");
                        self.vad_failed = true;
                    }
                    false
                }
            };
            if voiced {
                if let Some(timeline) = &self.timeline {
                    timeline
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .add(self.position, self.position + frame_samples);
                }
            }
            self.position += frame_samples;
            if let Some(utterance) = self.segmenter.push(frame, voiced) {
                self.submit(utterance);
            }
        }
        self.pending.drain(..frames * frame_samples);
    }

    fn submit(&self, utterance: Utterance) {
        let _ = self.jobs.send(Job {
            source: self.source,
            utterance,
        });
    }

    fn finish(mut self) {
        if let Some(utterance) = self.segmenter.finish() {
            self.submit(utterance);
        }
        if let Err(e) = self.wav.finalize() {
            error!("Finalizing meeting audio failed: {e}");
        }
    }
}

/// Transcribes utterances in order and stores them as segments.
struct Transcriber {
    app: AppHandle,
    store: Arc<MeetingStore>,
    meeting_id: i64,
    other_side_speech: Arc<Mutex<SpeechTimeline>>,
}

impl Transcriber {
    fn run(self, jobs: mpsc::Receiver<Job>) {
        while let Ok(job) = jobs.recv() {
            let utterance = &job.utterance;
            if job.source == Source::Mic && self.is_bleed(utterance) {
                debug!(
                    "Skipping a mic utterance at {}ms: the other side leaking into the microphone",
                    utterance.start_ms()
                );
                continue;
            }
            let Some(text) = transcribe(&self.app, &utterance.samples) else {
                continue;
            };
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            match self.store.add_segment(
                self.meeting_id,
                job.source,
                utterance.start_ms(),
                utterance.end_ms(),
                text,
            ) {
                Ok(segment) => {
                    if let Err(e) = (MeetingSegmentEvent { segment }).emit(&self.app) {
                        warn!("Failed to emit a meeting segment: {e}");
                    }
                }
                Err(e) => error!("Failed to store a meeting segment: {e}"),
            }
        }
    }

    fn is_bleed(&self, utterance: &Utterance) -> bool {
        let rms = (utterance.samples.iter().map(|s| s * s).sum::<f32>()
            / utterance.samples.len().max(1) as f32)
            .sqrt();
        rms < BLEED_MAX_RMS
            && self
                .other_side_speech
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .overlap(utterance.start, utterance.start + utterance.samples.len())
                >= BLEED_MIN_OVERLAP
    }
}

/// Transcribes with dictation's model, retrying a few times.
fn transcribe(app: &AppHandle, samples: &[f32]) -> Option<String> {
    let transcription = app.state::<Arc<TranscriptionManager>>();
    let recording = app.state::<Arc<AudioRecordingManager>>();
    for attempt in 1..=TRANSCRIBE_ATTEMPTS {
        // Dictation goes first: ONNX models serve one transcription at a
        // time, so wait out a dictation recording instead of racing it.
        while recording.is_recording() {
            std::thread::sleep(Duration::from_millis(200));
        }
        // The model may have been unloaded while idle between utterances.
        if !transcription.is_model_loaded() {
            transcription.initiate_model_load();
        }
        match transcription.transcribe(samples.to_vec()) {
            Ok(text) => return Some(text),
            Err(e) => {
                warn!(
                    "Meeting transcription failed (attempt {attempt}/{TRANSCRIBE_ATTEMPTS}): {e}"
                );
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    error!(
        "Dropping {}ms of meeting audio that could not be transcribed",
        samples_to_ms(samples.len())
    );
    None
}

/// Stops a recording when the app exits so its audio files are complete.
/// Utterances not transcribed yet are lost from the transcript (not the
/// audio): waiting for them could hold up quitting.
pub fn shutdown(app: &AppHandle) {
    let Some(manager) = app.try_state::<Arc<MeetingManager>>() else {
        return;
    };
    let mut session = match std::mem::replace(&mut *manager.lock_state(), State::Idle) {
        State::Recording(session) => session,
        _ => return,
    };
    let ended_at = session.stop_capture();
    if let Err(e) = manager.store.finish(session.meeting.id, ended_at) {
        error!("Failed to close meeting {}: {e}", session.meeting.id);
    }
}
