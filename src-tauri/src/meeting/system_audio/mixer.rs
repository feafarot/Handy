//! Puts audio from several output devices onto one timeline that follows the
//! wall clock, so it stays aligned with the microphone recording.
//!
//! Devices deliver nothing while idle and their clocks drift, so each one
//! keeps a [`Cursor`] that snaps back to the clock when it strays; the
//! [`Mixer`] sums what they deliver and emits it once it is old enough that
//! no late packet can still change it.

/// How far (in samples) a device may stray from the clock before its cursor
/// is snapped back.
pub const TOLERANCE: u64 = 16_000 / 5;
/// How far behind the clock emission trails; must exceed [`TOLERANCE`] so
/// that snapped packets never land on audio that was already emitted.
pub const LATENCY: u64 = 16_000 * 2 / 5;

/// Where one device's next samples go on the timeline.
pub struct Cursor {
    next: u64,
}

impl Cursor {
    pub fn starting_at(now: u64) -> Self {
        Self { next: now }
    }

    /// Timeline position for `len` samples that arrived at `now`.
    pub fn place(&mut self, len: usize, now: u64) -> u64 {
        let len = len as u64;
        let end = self.next + len;
        if end + TOLERANCE < now || end > now + TOLERANCE {
            self.next = now.saturating_sub(len);
        }
        let at = self.next;
        self.next += len;
        at
    }
}

/// Sums the devices' audio and hands it out in timeline order.
#[derive(Default)]
pub struct Mixer {
    /// Timeline samples already emitted.
    emitted: u64,
    /// Audio from `emitted` on.
    pending: Vec<f32>,
}

impl Mixer {
    /// Adds `samples` at timeline position `at`; any part that falls before
    /// what was already emitted is dropped.
    pub fn add(&mut self, at: u64, samples: &[f32]) {
        let skip = self.emitted.saturating_sub(at) as usize;
        let Some(samples) = samples.get(skip..) else {
            return;
        };
        let start = (at + skip as u64 - self.emitted) as usize;
        let end = start + samples.len();
        if self.pending.len() < end {
            self.pending.resize(end, 0.0);
        }
        for (mixed, sample) in self.pending[start..end].iter_mut().zip(samples) {
            *mixed += sample;
        }
    }

    /// Emits the timeline up to `until`, silence where nothing was added.
    pub fn emit(&mut self, until: u64, mut out: impl FnMut(&[f32])) {
        let Some(count) = until.checked_sub(self.emitted).filter(|&n| n > 0) else {
            return;
        };
        let count = count as usize;
        if self.pending.len() < count {
            self.pending.resize(count, 0.0);
        }
        out(&self.pending[..count]);
        self.pending.drain(..count);
        self.emitted = until;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(mixer: &mut Mixer, until: u64) -> Vec<f32> {
        let mut out = Vec::new();
        mixer.emit(until, |s| out.extend_from_slice(s));
        out
    }

    #[test]
    fn sums_overlapping_devices_and_fills_silence() {
        let mut mixer = Mixer::default();
        mixer.add(2, &[1.0, 1.0]);
        mixer.add(3, &[0.5, 0.5]);
        assert_eq!(collect(&mut mixer, 6), [0.0, 0.0, 1.0, 1.5, 0.5, 0.0]);
        assert_eq!(collect(&mut mixer, 8), [0.0, 0.0]);
    }

    #[test]
    fn drops_what_arrives_after_emission() {
        let mut mixer = Mixer::default();
        collect(&mut mixer, 4);
        mixer.add(2, &[1.0, 2.0, 3.0]);
        mixer.add(0, &[9.0]);
        assert_eq!(collect(&mut mixer, 6), [3.0, 0.0]);
    }

    #[test]
    fn cursor_follows_steady_delivery() {
        let mut cursor = Cursor::starting_at(1_000);
        assert_eq!(cursor.place(160, 1_160), 1_000);
        assert_eq!(cursor.place(160, 1_320), 1_160);
        // A little jitter is not corrected.
        assert_eq!(cursor.place(160, 1_400), 1_320);
    }

    #[test]
    fn cursor_snaps_after_idle_and_when_running_ahead() {
        let mut cursor = Cursor::starting_at(0);
        // Nothing played for 10 s: the packet ends now.
        assert_eq!(cursor.place(160, 160_000), 159_840);
        // A burst far ahead of the clock is pulled back as well.
        let at = cursor.place(TOLERANCE as usize * 2, 160_000);
        assert_eq!(at, 160_000 - TOLERANCE * 2);
    }
}
