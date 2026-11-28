//! How loud the music is right now, measured from the samples that leave Pulse.
//!
//! The UI cannot see the ring buffer: the samples are handed to the device in
//! a real-time callback and never come back. So the callback measures them on
//! the way out and publishes two numbers — a bass band and the full mix — that
//! a view can read once per frame.
//!
//! Everything here runs on the audio thread, which is why it is a pair of
//! one-pole filters and an envelope follower rather than an FFT: no
//! allocation, no locks, no branches worth counting. The filters are cheap
//! enough that measuring costs less than the limiter that may already run.
//!
//! The measurement is taken after volume, ReplayGain and any fade, so a cover
//! reacting to it follows what is actually coming out of the speakers rather
//! than what the file contains — a fade-out settles the visual too.
//!
//! Levels are read with relaxed ordering: they are a visual hint, and a frame
//! that sees the previous block's value is indistinguishable from one that
//! sees this block's.

use std::sync::atomic::{AtomicU32, Ordering};

/// Where the bass band ends. Below this is where a kick and a bassline live;
/// above it is everything that should not make a cover bounce.
pub const BASS_CUTOFF_HZ: f32 = 150.0;

/// Envelope times. Attack is short enough to catch a kick, release long enough
/// that the cover falls back rather than flickering between beats.
const ATTACK_MS: f32 = 12.0;
const RELEASE_MS: f32 = 220.0;

/// RMS that counts as "full" for each band. A loud master sits well under
/// these, so the visual has headroom instead of pinning at 1.0 on every track.
const BASS_FULL_SCALE: f32 = 0.12;
const MIX_FULL_SCALE: f32 = 0.25;

/// One frame's worth of levels, written by the audio thread and read by the UI.
#[derive(Debug, Default)]
pub struct Levels {
    bass: AtomicU32,
    mix: AtomicU32,
}

impl Levels {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bass band, 0.0–1.0, already enveloped.
    pub fn bass(&self) -> f32 {
        f32::from_bits(self.bass.load(Ordering::Relaxed))
    }

    /// The whole mix, 0.0–1.0, already enveloped.
    pub fn mix(&self) -> f32 {
        f32::from_bits(self.mix.load(Ordering::Relaxed))
    }

    /// Drop both readings to zero. Used when the stream is torn down, so a
    /// paused player does not leave a cover stuck mid-bounce.
    pub fn clear(&self) {
        self.bass.store(0.0f32.to_bits(), Ordering::Relaxed);
        self.mix.store(0.0f32.to_bits(), Ordering::Relaxed);
    }

    fn publish(&self, bass: f32, mix: f32) {
        self.bass.store(bass.to_bits(), Ordering::Relaxed);
        self.mix.store(mix.to_bits(), Ordering::Relaxed);
    }
}

/// Two cascaded one-poles. -12 dB/octave is not a brick wall, but it is enough
/// to keep a vocal out of a bass reading, and it costs two multiply-adds.
#[derive(Debug, Clone, Copy)]
struct LowPass {
    coefficient: f32,
    first: f32,
    second: f32,
}

impl LowPass {
    fn new(cutoff_hz: f32, sample_rate: f32) -> Self {
        let coefficient = if sample_rate > 0.0 {
            1.0 - (-std::f32::consts::TAU * cutoff_hz / sample_rate).exp()
        } else {
            1.0
        };
        Self {
            coefficient,
            first: 0.0,
            second: 0.0,
        }
    }

    fn process(&mut self, sample: f32) -> f32 {
        self.first += self.coefficient * (sample - self.first);
        self.second += self.coefficient * (self.first - self.second);
        self.second
    }

    fn reset(&mut self) {
        self.first = 0.0;
        self.second = 0.0;
    }
}

/// The analyser. One per stream, owned by the callback: it holds the filter and
/// envelope state that must persist between blocks.
#[derive(Debug, Clone)]
pub struct LevelMeter {
    lowpass: LowPass,
    sample_rate: f32,
    channels: usize,
    bass: f32,
    mix: f32,
}

impl LevelMeter {
    pub fn new(sample_rate: f32, channels: usize) -> Self {
        Self {
            lowpass: LowPass::new(BASS_CUTOFF_HZ, sample_rate),
            sample_rate,
            channels: channels.max(1),
            bass: 0.0,
            mix: 0.0,
        }
    }

    /// Measure one callback's worth of interleaved samples and publish the
    /// result. `data` is exactly what the device is about to play.
    pub fn observe(&mut self, data: &[f32], levels: &Levels) {
        let frames = data.len() / self.channels;
        if frames == 0 {
            return;
        }

        let mut bass_energy = 0.0f32;
        let mut mix_energy = 0.0f32;
        for frame in data.chunks_exact(self.channels) {
            let mut mono = 0.0;
            for sample in frame {
                mono += *sample;
            }
            mono /= self.channels as f32;
            let low = self.lowpass.process(mono);
            bass_energy += low * low;
            mix_energy += mono * mono;
        }

        let count = frames as f32;
        let bass_rms = (bass_energy / count).sqrt();
        let mix_rms = (mix_energy / count).sqrt();
        // The block's duration is what the envelope has to move over, so a
        // large buffer and a small one settle at the same speed.
        let seconds = count / self.sample_rate.max(1.0);

        self.bass = envelope(self.bass, normalise(bass_rms, BASS_FULL_SCALE), seconds);
        self.mix = envelope(self.mix, normalise(mix_rms, MIX_FULL_SCALE), seconds);
        levels.publish(self.bass, self.mix);
    }

    /// Forget the last track's tail, so the next one starts from silence.
    pub fn reset(&mut self) {
        self.lowpass.reset();
        self.bass = 0.0;
        self.mix = 0.0;
    }
}

/// RMS to 0.0–1.0. The square root is deliberate: loudness is perceived
/// logarithmically, and without it quiet tracks barely move the visual.
fn normalise(rms: f32, full_scale: f32) -> f32 {
    (rms / full_scale).min(1.0).sqrt()
}

/// Attack fast, release slow, with the coefficient derived from the block's
/// real duration.
fn envelope(current: f32, target: f32, seconds: f32) -> f32 {
    let time_ms = if target > current { ATTACK_MS } else { RELEASE_MS };
    let coefficient = 1.0 - (-seconds * 1000.0 / time_ms).exp();
    current + (target - current) * coefficient.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: f32 = 48_000.0;

    /// Feed `seconds` of a sine in 20 ms blocks, the way the device asks for it.
    fn feed(meter: &mut LevelMeter, levels: &Levels, hz: f32, amplitude: f32, seconds: f32) {
        let block = (SAMPLE_RATE * 0.02) as usize;
        let total = (SAMPLE_RATE * seconds) as usize;
        let mut data = vec![0.0f32; block * 2];
        let mut produced = 0;
        let mut phase = 0.0f32;
        let step = std::f32::consts::TAU * hz / SAMPLE_RATE;
        while produced < total {
            for frame in data.as_chunks_mut::<2>().0 {
                let sample = phase.sin() * amplitude;
                frame[0] = sample;
                frame[1] = sample;
                phase = (phase + step) % std::f32::consts::TAU;
            }
            meter.observe(&data, levels);
            produced += block;
        }
    }

    #[test]
    fn silence_reads_zero() {
        let levels = Levels::new();
        let mut meter = LevelMeter::new(SAMPLE_RATE, 2);
        meter.observe(&[0.0; 2048], &levels);
        assert_eq!(levels.bass(), 0.0);
        assert_eq!(levels.mix(), 0.0);
    }

    #[test]
    fn a_kick_moves_the_bass() {
        let levels = Levels::new();
        let mut meter = LevelMeter::new(SAMPLE_RATE, 2);
        feed(&mut meter, &levels, 60.0, 0.5, 0.2);
        assert!(levels.bass() > 0.5, "bass was {}", levels.bass());
        assert!(levels.mix() > 0.3, "mix was {}", levels.mix());
    }

    #[test]
    fn a_hi_hat_does_not() {
        let levels = Levels::new();
        let mut meter = LevelMeter::new(SAMPLE_RATE, 2);
        feed(&mut meter, &levels, 8_000.0, 0.5, 0.2);
        assert!(levels.bass() < 0.1, "bass was {}", levels.bass());
        assert!(levels.mix() > 0.3, "mix was {}", levels.mix());
    }

    #[test]
    fn the_bass_falls_back_after_the_beat() {
        let levels = Levels::new();
        let mut meter = LevelMeter::new(SAMPLE_RATE, 2);
        feed(&mut meter, &levels, 60.0, 0.5, 0.2);
        let peak = levels.bass();
        assert!(peak > 0.5);

        feed(&mut meter, &levels, 60.0, 0.0, 0.1);
        let early = levels.bass();
        assert!(early < peak, "should have started falling: {early} vs {peak}");
        assert!(early > 0.0, "release should not be instant");

        feed(&mut meter, &levels, 60.0, 0.0, 1.0);
        assert!(levels.bass() < 0.1, "should be nearly gone: {}", levels.bass());
    }

    #[test]
    fn mono_and_stereo_measure_the_same() {
        let stereo = Levels::new();
        let mut stereo_meter = LevelMeter::new(SAMPLE_RATE, 2);
        feed(&mut stereo_meter, &stereo, 60.0, 0.5, 0.2);

        let mono = Levels::new();
        let mut mono_meter = LevelMeter::new(SAMPLE_RATE, 1);
        let block = (SAMPLE_RATE * 0.02) as usize;
        let mut data = vec![0.0f32; block];
        let mut phase = 0.0f32;
        let step = std::f32::consts::TAU * 60.0 / SAMPLE_RATE;
        for _ in 0..10 {
            for sample in data.iter_mut() {
                *sample = phase.sin() * 0.5;
                phase = (phase + step) % std::f32::consts::TAU;
            }
            mono_meter.observe(&data, &mono);
        }
        assert!((mono.bass() - stereo.bass()).abs() < 0.05);
    }

    #[test]
    fn a_partial_block_is_not_a_panic() {
        let levels = Levels::new();
        let mut meter = LevelMeter::new(SAMPLE_RATE, 2);
        // One and a half frames: the trailing sample has no partner.
        meter.observe(&[0.5, 0.5, 0.5], &levels);
        assert!(levels.mix() >= 0.0);
    }

    #[test]
    fn clearing_drops_the_reading() {
        let levels = Levels::new();
        let mut meter = LevelMeter::new(SAMPLE_RATE, 2);
        feed(&mut meter, &levels, 60.0, 0.5, 0.2);
        assert!(levels.bass() > 0.0);
        levels.clear();
        assert_eq!(levels.bass(), 0.0);
        assert_eq!(levels.mix(), 0.0);
    }
}
