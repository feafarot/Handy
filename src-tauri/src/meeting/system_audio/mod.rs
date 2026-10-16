//! Recording what the PC plays (the other side of a call), as 16 kHz mono.
//!
//! Each platform provides `SystemAudioCapture::start(AudioCallback)`, stopping
//! on drop; platforms without one record the microphone only.

/// Receives captured audio on the capture thread; must return promptly.
pub type AudioCallback = Box<dyn FnMut(&[f32]) + Send>;

#[cfg(any(target_os = "windows", test))]
mod mixer;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::SystemAudioCapture;

#[cfg(not(target_os = "windows"))]
mod unsupported {
    use super::AudioCallback;

    pub struct SystemAudioCapture;

    impl SystemAudioCapture {
        pub fn start(_on_audio: AudioCallback) -> Result<Self, String> {
            Err("system audio capture is not supported on this platform".into())
        }
    }
}
#[cfg(not(target_os = "windows"))]
pub use unsupported::SystemAudioCapture;

/// Whether this platform can record system audio at all.
pub const SUPPORTED: bool = cfg!(target_os = "windows");
