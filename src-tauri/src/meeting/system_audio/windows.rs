//! System audio on Windows: WASAPI loopback of every active output device,
//! converted to 16 kHz mono by Windows and summed onto one timeline.
//!
//! Process loopback (which could leave Handy's own sounds out) is not used:
//! it silently drops streams tagged as communications, which is how Teams,
//! Zoom and the like play call audio. So Handy's feedback sounds end up in
//! the recording too. Audio a virtual device (e.g. Voicemeeter) forwards to
//! a physical one is heard on both and summed twice.
//!
//! Devices are polled rather than event driven, and re-enumerated every few
//! seconds so headsets plugged in mid-meeting are picked up.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eRender, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};

use super::mixer::{Cursor, Mixer, LATENCY};
use super::AudioCallback;
use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE as SAMPLE_RATE;

const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
/// WASAPI buffer, in 100 ns units: one second, so opening a newly plugged
/// in device does not overrun the others.
const BUFFER_DURATION: i64 = 10_000_000;
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const REFRESH_INTERVAL: Duration = Duration::from_secs(3);

/// A running capture; stops when dropped.
pub struct SystemAudioCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SystemAudioCapture {
    /// Starts capturing, returning once the devices are open or none could be.
    pub fn start(on_audio: AudioCallback) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("system-audio".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || run(on_audio, &stop, ready_tx)
            })
            .map_err(|e| format!("failed to spawn the capture thread: {e}"))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err("the capture thread exited before starting".into())
            }
        }
    }
}

impl Drop for SystemAudioCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(mut on_audio: AudioCallback, stop: &AtomicBool, ready: mpsc::Sender<Result<(), String>>) {
    // SAFETY: balanced by the CoUninitialize below, on this thread.
    if let Err(e) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
        let _ = ready.send(Err(format!("COM initialization failed: {e}")));
        return;
    }
    // Scoped so every COM object is released before CoUninitialize.
    {
        // SAFETY: plain COM activation on an initialized thread.
        let enumerator: windows::core::Result<IMMDeviceEnumerator> =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) };
        let mut devices = match enumerator {
            Ok(enumerator) => Devices::new(enumerator),
            Err(e) => {
                let _ = ready.send(Err(format!("listing output devices failed: {e}")));
                unsafe { CoUninitialize() };
                return;
            }
        };
        devices.refresh(0);
        if devices.open.is_empty() {
            let _ = ready.send(Err("no output device could be captured".into()));
        } else {
            // The timeline starts once the devices run, right before the
            // caller starts the microphone.
            let started = Instant::now();
            let now = || (started.elapsed().as_secs_f64() * f64::from(SAMPLE_RATE)) as u64;
            let _ = ready.send(Ok(()));
            let mut mixer = Mixer::default();
            let mut refreshed = Instant::now();
            while !stop.load(Ordering::Acquire) {
                std::thread::sleep(POLL_INTERVAL);
                devices.drain(now(), &mut mixer);
                if refreshed.elapsed() >= REFRESH_INTERVAL {
                    devices.refresh(now());
                    refreshed = Instant::now();
                }
                mixer.emit(now().saturating_sub(LATENCY), &mut on_audio);
            }
            devices.drain(now(), &mut mixer);
            mixer.emit(now(), &mut on_audio);
        }
    }
    unsafe { CoUninitialize() };
}

/// The output devices being captured.
struct Devices {
    enumerator: IMMDeviceEnumerator,
    open: Vec<Endpoint>,
    /// Devices that could not be opened, so the failure is logged only once.
    failed: HashSet<String>,
}

impl Devices {
    fn new(enumerator: IMMDeviceEnumerator) -> Self {
        Self {
            enumerator,
            open: Vec::new(),
            failed: HashSet::new(),
        }
    }

    /// Opens newly active devices and drops the ones that went away.
    fn refresh(&mut self, now: u64) {
        let active = match self.active() {
            Ok(active) => active,
            Err(e) => {
                log::warn!("Listing output devices failed: {e}");
                return;
            }
        };
        self.open
            .retain(|endpoint| active.iter().any(|(id, _)| *id == endpoint.id));
        for (id, device) in active {
            if self.open.iter().any(|endpoint| endpoint.id == id) {
                continue;
            }
            let name = friendly_name(&device);
            match Endpoint::open(&device, id.clone(), now) {
                Ok(endpoint) => {
                    log::info!("Capturing system audio from {name}");
                    self.failed.remove(&id);
                    self.open.push(endpoint);
                }
                Err(e) => {
                    if self.failed.insert(id) {
                        log::warn!("Cannot capture system audio from {name}: {e}");
                    }
                }
            }
        }
    }

    fn active(&self) -> windows::core::Result<Vec<(String, IMMDevice)>> {
        // SAFETY: plain COM calls; each id string is copied, then freed once.
        unsafe {
            let collection = self
                .enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            let mut active = Vec::new();
            for i in 0..collection.GetCount()? {
                let device = collection.Item(i)?;
                let id = device.GetId()?;
                let id_string = id.to_string().unwrap_or_default();
                CoTaskMemFree(Some(id.0 as *const _));
                active.push((id_string, device));
            }
            Ok(active)
        }
    }

    /// Moves every device's pending audio into the mixer, dropping devices
    /// that failed (unplugged, reconfigured).
    fn drain(&mut self, now: u64, mixer: &mut Mixer) {
        self.open
            .retain_mut(|endpoint| match endpoint.drain(now, mixer) {
                Ok(()) => true,
                Err(e) => {
                    log::info!("Stopped capturing an output device: {e}");
                    false
                }
            });
    }
}

fn friendly_name(device: &IMMDevice) -> String {
    // SAFETY: plain COM calls on a live device.
    unsafe {
        device
            .OpenPropertyStore(STGM_READ)
            .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
            .map(|name| name.to_string())
            .unwrap_or_else(|_| "an unnamed output device".into())
    }
}

/// Loopback of one output device.
struct Endpoint {
    id: String,
    client: IAudioClient,
    capture: IAudioCaptureClient,
    cursor: Cursor,
}

impl Endpoint {
    fn open(device: &IMMDevice, id: String, now: u64) -> windows::core::Result<Self> {
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: 1,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * 4,
            nBlockAlign: 4,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        // SAFETY: plain COM calls on a client this thread owns; `format`
        // outlives Initialize.
        unsafe {
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                BUFFER_DURATION,
                0,
                &format,
                None,
            )?;
            let capture = client.GetService()?;
            client.Start()?;
            Ok(Self {
                id,
                client,
                capture,
                cursor: Cursor::starting_at(now),
            })
        }
    }

    fn drain(&mut self, now: u64, mixer: &mut Mixer) -> windows::core::Result<()> {
        // SAFETY: GetBuffer/ReleaseBuffer are paired, and the buffer is only
        // read in between.
        unsafe {
            while self.capture.GetNextPacketSize()? > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0;
                let mut flags = 0;
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let at = self.cursor.place(frames as usize, now);
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 == 0 && !data.is_null() {
                    mixer.add(
                        at,
                        std::slice::from_raw_parts(data.cast::<f32>(), frames as usize),
                    );
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
        Ok(())
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        // SAFETY: the client is still alive; nothing uses it after this.
        unsafe {
            let _ = self.client.Stop();
        }
    }
}
