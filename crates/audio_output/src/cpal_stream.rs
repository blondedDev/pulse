pub use crate::ring_buffer::AudioRingBuffer;

use atomic_float::AtomicF32;
use audio_common::{AudioBatch, AudioError};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, OutputCallbackInfo, SampleFormat, SizedSample, Stream, StreamConfig};
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::FadeEvent;

#[derive(Clone, Copy, Debug)]
pub struct OutputConfig {
    pub sample_rate: u32,
    pub channels: u8,
    pub bit_depth: u8,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            channels: 2,
            sample_rate: 44100,
            bit_depth: 32,
        }
    }
}

/// The loudest a volume slider may be pushed to, as a multiple of unity gain.
///
/// 1.0 is the file's own level. Above it the player is deliberately adding
/// gain, so the shared path runs a peak limiter (see [`LimiterState`]) to keep
/// the extra level from turning into hard clipping at the device. 1.5 is about
/// +3.5 dB — enough for a quiet master without the limiter working constantly.
pub const MAX_VOLUME: f32 = 1.5;

pub trait AudioOutput: Send + Sync {
    fn write(&self, samples: &AudioBatch) -> usize;
    fn clear(&self);
    fn pause(&self);
    fn resume(&self);
    fn is_playing(&self) -> bool;
    fn set_volume(&self, volume: f32);
    /// Samples written but not yet consumed by the device.
    ///
    /// A format change has to rebuild the stream, and the rebuilt stream gets a
    /// fresh ring buffer — whatever was pending is lost with it. The engine uses
    /// this to let the outgoing track's tail play out before the swap.
    fn pending_samples(&self) -> usize;
}

pub struct SelectedOutputDevice {
    pub host: Arc<cpal::Host>,
    pub device: Arc<cpal::Device>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Idle,
    Playing,
    Paused,
}

struct CpalOutputStreamInner {
    state: PlaybackState,
    stream: Stream,
}

pub struct CpalOutputStream {
    inner: RwLock<CpalOutputStreamInner>,
    buffer: Arc<AudioRingBuffer>,
    volume: Arc<AtomicF32>,
    /// ReplayGain for the track being played, applied in the callback next to
    /// the volume. Kept separate so neither overwrites the other.
    track_gain: Arc<AtomicF32>,
    fade: Arc<FadeState>,
    silenced: Arc<AtomicBool>,
    device_lost: Arc<AtomicBool>,
    limiter: Arc<LimiterState>,
    pub config: OutputConfig,
}

/// Lock-free fade envelope shared between the control side (`begin_fade`) and
/// the real-time callback. The callback ramps `gain` toward `target` by `step`
/// (once per frame) and posts an `event` when it lands on the target.
pub(crate) struct FadeState {
    gain: AtomicF32,
    step: AtomicF32,
    target: AtomicF32,
    event: AtomicU8,
}

impl FadeState {
    pub(crate) fn new() -> Self {
        Self {
            gain: AtomicF32::new(1.0),
            step: AtomicF32::new(0.0),
            target: AtomicF32::new(1.0),
            event: AtomicU8::new(0),
        }
    }

    /// Starts a ramp toward `target` over `duration_ms`. If `start` is given,
    /// the gain jumps there first (used to fade in from silence on resume).
    pub(crate) fn begin(
        &self,
        sample_rate: u32,
        start: Option<f32>,
        target: f32,
        duration_ms: u32,
    ) {
        if let Some(s) = start {
            self.gain.store(s, Ordering::SeqCst);
        }
        let from = self.gain.load(Ordering::SeqCst);
        let frames = sample_rate as f32 * (duration_ms as f32 / 1000.0);
        let mag = if frames >= 1.0 { 1.0 / frames } else { 1.0 };
        let step = if target >= from { mag } else { -mag };
        self.target.store(target, Ordering::SeqCst);
        self.event.store(0, Ordering::SeqCst);
        // Store step last so the callback never sees a stale target/gain.
        self.step.store(step, Ordering::SeqCst);
    }

    pub(crate) fn take_event(&self) -> Option<FadeEvent> {
        match self.event.swap(0, Ordering::SeqCst) {
            1 => Some(FadeEvent::FadedIn),
            2 => Some(FadeEvent::FadedOut),
            _ => None,
        }
    }

    /// Cancels any ramp and pins the gain at unity. Used when (re)loading or
    /// stopping a track so fresh content never inherits a stale/frozen gain.
    pub(crate) fn reset(&self) {
        self.step.store(0.0, Ordering::SeqCst);
        self.target.store(1.0, Ordering::SeqCst);
        self.gain.store(1.0, Ordering::SeqCst);
        self.event.store(0, Ordering::SeqCst);
    }

    /// A completed fade-out (gain pinned at 0 with no active ramp) means the
    /// callback should emit silence WITHOUT draining the ring buffer, so the
    /// un-played samples survive for a seamless fade-in on resume.
    pub(crate) fn is_frozen(&self) -> bool {
        self.gain.load(Ordering::Relaxed) == 0.0 && self.step.load(Ordering::Relaxed) == 0.0
    }
}

/// A zero-lookahead peak limiter for the volume above unity.
///
/// Below 100% nothing here runs and the signal path is unchanged. Above it the
/// post-volume samples are scanned a frame at a time; any frame hotter than
/// full scale pulls the gain down immediately, and it eases back toward unity
/// over [`LIMITER_RELEASE_SECS`]. Instant attack means the very frame that
/// overloaded the output is scaled down before it reaches the device, so the
/// result is quieter rather than clipped. With no lookahead the gain jump can
/// round the corner of a transient (audible as compression, not as the crackle
/// of clipping); that is the trade for a limiter that costs one pass and no
/// delay.
pub(crate) struct LimiterState {
    gain: AtomicF32,
    /// Per-frame release coefficient, derived from the stream's sample rate.
    release: f32,
}

/// How long the limiter takes to climb back toward unity once the signal gets
/// quiet again. Short enough not to pump, long enough not to distort a kick
/// drum's decay.
const LIMITER_RELEASE_SECS: f32 = 0.15;

impl LimiterState {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let release = 1.0 - (-1.0 / (LIMITER_RELEASE_SECS * sample_rate as f32)).exp();
        Self {
            gain: AtomicF32::new(1.0),
            release,
        }
    }

    /// Back to unity, for when the volume drops to 100% or below again.
    pub(crate) fn reset(&self) {
        self.gain.store(1.0, Ordering::SeqCst);
    }
}

/// Scales `buf` down frame by frame so no sample leaves above full scale.
///
/// Interleaved frames share one gain so the stereo image does not shift; a
/// quiet buffer leaves the envelope where it is and recovers gradually.
pub(crate) fn apply_limiter(buf: &mut [f32], channels: usize, limiter: &LimiterState) {
    let mut gain = limiter.gain.load(Ordering::Relaxed);
    let channels = channels.max(1);
    for frame in buf.chunks_mut(channels) {
        let peak = frame
            .iter()
            .fold(0.0f32, |max, sample| max.max(sample.abs()));
        let target = if peak > 1.0 { 1.0 / peak } else { 1.0 };
        if target < gain {
            gain = target;
        } else {
            gain += (target - gain) * limiter.release;
            if gain > 1.0 {
                gain = 1.0;
            }
        }
        for sample in frame.iter_mut() {
            *sample *= gain;
        }
    }
    limiter.gain.store(gain, Ordering::Relaxed);
}

/// The three multipliers a callback applies, kept together so the render
/// function stays a readable list of steps.
struct Gains<'a> {
    volume: &'a AtomicF32,
    track_gain: &'a AtomicF32,
    limiter: &'a LimiterState,
}

/// Applies `base_volume` and any active fade ramp to an interleaved buffer,
/// stepping the fade gain once per frame and signalling completion.
pub(crate) fn apply_fade_gain(
    fade: &FadeState,
    base_volume: f32,
    channels: usize,
    buf: &mut [f32],
) {
    let mut gain = fade.gain.load(Ordering::Relaxed);
    let step = fade.step.load(Ordering::Relaxed);

    if step == 0.0 {
        // Steady gain: skip the multiply near unity so bit-perfect output is
        // preserved (matches the tolerance the bit-perfect indicator uses).
        // Above unity must still be applied — that is the volume boost.
        let m = base_volume * gain;
        if (m - 1.0).abs() <= crate::bit_perfect::UNITY_VOLUME_TOLERANCE {
            return;
        }
        for s in buf.iter_mut() {
            *s *= m;
        }
        return;
    }

    let target = fade.target.load(Ordering::Relaxed);
    let ch = channels.max(1);
    let mut completed = 0u8;
    for (i, s) in buf.iter_mut().enumerate() {
        *s *= base_volume * gain;
        if completed == 0 && i % ch == ch - 1 {
            gain += step;
            if (step > 0.0 && gain >= target) || (step < 0.0 && gain <= target) {
                gain = target;
                completed = if target <= 0.0 { 2 } else { 1 };
            }
        }
    }
    fade.gain.store(gain, Ordering::Relaxed);
    if completed != 0 {
        fade.step.store(0.0, Ordering::Relaxed);
        fade.event.store(completed, Ordering::Relaxed);
    }
}

/// Renders one callback's worth of audio into an interleaved f32 `data` slice:
/// emits silence while frozen mid-fade, otherwise drains the ring buffer,
/// applies volume + fade gain, and zero-pads any underrun tail. Shared by the
/// native-f32 callback and the format-converting callbacks below.
fn fill_f32(
    fade: &FadeState,
    silenced: &AtomicBool,
    buffer: &AudioRingBuffer,
    gains: Gains<'_>,
    channels: usize,
    data: &mut [f32],
) {
    // Frozen after a fade-out: emit silence but leave the buffer intact so
    // resume can fade those same samples back in seamlessly.
    if silenced.load(Ordering::Acquire) || fade.is_frozen() {
        for sample in data.iter_mut() {
            *sample = 0.0;
        }
        return;
    }

    let samples_read = buffer.pop_slice(data);
    // One multiply for both gains: `apply_fade_gain` keeps its unity fast path,
    // so ReplayGain off + volume at 100% is still bit-identical.
    let user_volume = gains.volume.load(Ordering::Relaxed);
    let vol = user_volume * gains.track_gain.load(Ordering::Relaxed);
    apply_fade_gain(fade, vol, channels, &mut data[..samples_read]);
    // Above 100% the extra gain can push samples past full scale. The limiter
    // only exists for that case: at or below unity the samples are untouched,
    // so the bit-perfect path stays bit-perfect and ReplayGain keeps its own
    // (peak-aware) behaviour.
    if user_volume > 1.0 {
        apply_limiter(&mut data[..samples_read], channels, gains.limiter);
    }
    for sample in &mut data[samples_read..] {
        *sample = 0.0;
    }
}

fn on_stream_error(err: cpal::StreamError, device_lost: &AtomicBool) {
    log::error!("Audio stream error: {}", err);
    if !matches!(err, cpal::StreamError::BufferUnderrun) {
        device_lost.store(true, Ordering::SeqCst);
    }
}

/// Picks the sample format to open the device with. Prefers the device's own
/// default format: it's F32 on the common shared path (no conversion, keeps the
/// signal intact) but is an integer format (e.g. I32/I16) on digital outputs
/// like S/PDIF and HDMI that reject F32. We then convert our internal f32
/// samples to that format in the callback.
fn negotiate_sample_format(device: &cpal::Device) -> SampleFormat {
    device
        .default_output_config()
        .map(|c| c.sample_format())
        .unwrap_or(SampleFormat::F32)
}

#[derive(Clone)]
struct RenderState {
    buffer: Arc<AudioRingBuffer>,
    volume: Arc<AtomicF32>,
    fade: Arc<FadeState>,
    silenced: Arc<AtomicBool>,
    /// ReplayGain for the track being played. Separate from `volume` so a
    /// user turning the volume down never overwrites it, and so the user's
    /// level survives a track change.
    track_gain: Arc<AtomicF32>,
    limiter: Arc<LimiterState>,
}

impl RenderState {
    fn fill(&self, channels: usize, data: &mut [f32]) {
        fill_f32(
            &self.fade,
            &self.silenced,
            &self.buffer,
            Gains {
                volume: &self.volume,
                track_gain: &self.track_gain,
                limiter: &self.limiter,
            },
            channels,
            data,
        );
    }
}

/// Builds an output stream whose hardware sample type is `T`, converting each
/// rendered f32 sample to `T`. A reusable scratch buffer holds the f32 frame so
/// the shared `fill_f32` logic stays format-agnostic.
fn build_converting_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    render: RenderState,
    device_lost: Arc<AtomicBool>,
    channels: usize,
) -> Result<Stream, cpal::BuildStreamError>
where
    T: SizedSample + FromSample<f32>,
{
    // Pre-allocate the f32 scratch buffer outside the realtime callback so the
    // hot path stays allocation-free: `resize` only reallocs if it has to grow
    // past capacity, so reserve a generous worst-case period up front. (cpal can
    // still hand a larger buffer than this in rare configs, costing one realloc;
    // that's the only remaining allocation and it doesn't repeat.)
    const SCRATCH_PREALLOC: usize = 1 << 15; // 32768 f32 samples (16384 stereo frames)
    let mut scratch: Vec<f32> = Vec::with_capacity(SCRATCH_PREALLOC);
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &OutputCallbackInfo| {
            if scratch.len() != data.len() {
                scratch.resize(data.len(), 0.0);
            }
            render.fill(channels, &mut scratch);
            for (out, sample) in data.iter_mut().zip(scratch.iter()) {
                *out = T::from_sample(*sample);
            }
        },
        move |err| on_stream_error(err, &device_lost),
        None,
    )
}

const TARGET_PERIOD_MS: u32 = 20;

fn preferred_period(device: &cpal::Device, config: &OutputConfig) -> Option<u32> {
    use cpal::traits::DeviceTrait;

    let frames = (config.sample_rate * TARGET_PERIOD_MS / 1000).next_power_of_two();
    match device.default_output_config().ok()?.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => Some(frames.clamp(*min, *max)),
        cpal::SupportedBufferSize::Unknown => None,
    }
}

impl CpalOutputStream {
    /// ReplayGain for the current track, as a linear multiplier.
    ///
    /// Not part of `AudioOutput`: it only exists on the shared (software) path.
    /// Native-sample-rate mode is untouched by design — it is the output that
    /// promises the file's samples reach the device unmodified.
    pub fn set_track_gain(&self, gain: f32) {
        self.track_gain
            .store(gain.clamp(0.0, 4.0), Ordering::Relaxed);
    }

    pub fn new(
        buffer: Arc<AudioRingBuffer>,
        output_config: OutputConfig,
        device: SelectedOutputDevice,
    ) -> Result<Self, AudioError> {
        let volume = Arc::new(AtomicF32::new(1.0));
        let track_gain = Arc::new(AtomicF32::new(1.0));
        let fade = Arc::new(FadeState::new());
        let silenced = Arc::new(AtomicBool::new(true));
        let limiter = Arc::new(LimiterState::new(output_config.sample_rate));
        let render = RenderState {
            buffer: buffer.clone(),
            volume: volume.clone(),
            track_gain: track_gain.clone(),
            fade: fade.clone(),
            silenced: silenced.clone(),
            limiter: limiter.clone(),
        };
        let device_lost = Arc::new(AtomicBool::new(false));
        let channels = output_config.channels as usize;

        let stream_config = |buffer_size: cpal::BufferSize| StreamConfig {
            channels: output_config.channels as u16,
            sample_rate: output_config.sample_rate,
            buffer_size,
        };

        let dev = &device.device;
        let format = negotiate_sample_format(dev);

        // F32 takes a direct path (no per-sample conversion); integer formats
        // go through the converting builder.
        let build = |stream_config: &StreamConfig| -> Result<Stream, cpal::BuildStreamError> {
            match format {
                SampleFormat::F32 => {
                    let (render, device_lost) = (render.clone(), device_lost.clone());
                    dev.build_output_stream(
                        stream_config,
                        move |data: &mut [f32], _: &OutputCallbackInfo| {
                            render.fill(channels, data);
                        },
                        move |err| on_stream_error(err, &device_lost),
                        None,
                    )
                }
                SampleFormat::I32 => build_converting_stream::<i32>(
                    dev,
                    stream_config,
                    render.clone(),
                    device_lost.clone(),
                    channels,
                ),
                SampleFormat::I16 => build_converting_stream::<i16>(
                    dev,
                    stream_config,
                    render.clone(),
                    device_lost.clone(),
                    channels,
                ),
                SampleFormat::U16 => build_converting_stream::<u16>(
                    dev,
                    stream_config,
                    render.clone(),
                    device_lost.clone(),
                    channels,
                ),
                SampleFormat::U8 => build_converting_stream::<u8>(
                    dev,
                    stream_config,
                    render.clone(),
                    device_lost.clone(),
                    channels,
                ),
                // Formats we don't convert to (I24, I64, F64, …). Surface as an
                // error; the caller installs a fallback rather than dying.
                _ => Err(cpal::BuildStreamError::StreamConfigNotSupported),
            }
        };

        let fixed = preferred_period(dev, &output_config).and_then(|frames| {
            match build(&stream_config(cpal::BufferSize::Fixed(frames))) {
                Ok(stream) => Some(stream),
                Err(e) => {
                    log::warn!(
                        "audio output: device rejected a {frames}-frame period ({e}); falling back to its default"
                    );
                    None
                }
            }
        });

        let output_stream = match fixed {
            Some(stream) => stream,
            None => build(&stream_config(cpal::BufferSize::Default))
                .map_err(|e| AudioError::Output(format!("{e} (sample format {format:?})")))?,
        };

        Ok(Self {
            inner: RwLock::new(CpalOutputStreamInner {
                state: PlaybackState::Idle,
                stream: output_stream,
            }),
            buffer: buffer.clone(),
            config: output_config,
            volume,
            track_gain,
            fade,
            silenced,
            device_lost,
            limiter,
        })
    }

    pub fn take_device_lost(&self) -> bool {
        self.device_lost.swap(false, Ordering::SeqCst)
    }

    /// Starts a fade ramp toward `target` (0.0 = out, 1.0 = in) over
    /// `duration_ms`. `start`, if given, seeds the gain before ramping.
    pub fn begin_fade(&self, start: Option<f32>, target: f32, duration_ms: u32) {
        self.fade
            .begin(self.config.sample_rate, start, target, duration_ms);
    }

    /// Returns and clears a pending fade-completion signal from the callback.
    pub fn take_fade_event(&self) -> Option<FadeEvent> {
        self.fade.take_event()
    }

    /// Cancels any active fade and restores full gain.
    pub fn reset_fade(&self) {
        self.fade.reset();
    }

    pub fn release(&self) {
        let inner = self.inner.read();
        if inner.state == PlaybackState::Playing {
            return;
        }
        if let Err(e) = inner.stream.pause() {
            log::error!("audio output: failed to stop idle stream: {}", e);
        }
    }
}

impl AudioOutput for CpalOutputStream {
    fn write(&self, samples: &AudioBatch) -> usize {
        if self.inner.read().state != PlaybackState::Playing {
            return 0;
        }

        let f32_samples = samples.data.to_f32();
        self.buffer.write_slice_blocking(&f32_samples)
    }

    fn clear(&self) {
        self.buffer.clear();
    }

    fn pause(&self) {
        let mut inner = self.inner.write();
        match inner.state {
            PlaybackState::Playing => {
                self.silenced.store(true, Ordering::SeqCst);
                inner.state = PlaybackState::Paused;
            }
            PlaybackState::Idle => {
                if let Err(e) = inner.stream.pause() {
                    log::error!("audio output: failed to stop unused stream: {}", e);
                }
            }
            PlaybackState::Paused => {}
        }
    }

    fn resume(&self) {
        let mut inner = self.inner.write();
        if inner.state == PlaybackState::Playing {
            return;
        }
        self.silenced.store(false, Ordering::SeqCst);
        if let Err(e) = inner.stream.play() {
            log::error!("audio output: failed to resume stream: {}", e);
        }
        inner.state = PlaybackState::Playing;
    }

    fn is_playing(&self) -> bool {
        self.inner.read().state == PlaybackState::Playing
    }

    fn set_volume(&self, value: f32) {
        if !(0.0..=MAX_VOLUME).contains(&value) {
            panic!("Volume must be between 0.0 and {MAX_VOLUME}");
        }

        let calculated_value = calculate_volume_scaled(value);
        self.volume.store(calculated_value, Ordering::SeqCst);
        // The limiter belongs to the boost. Coming back to 100% or below gets
        // the untouched signal path immediately, not one release time later.
        if calculated_value <= 1.0 {
            self.limiter.reset();
        }
    }

    fn pending_samples(&self) -> usize {
        self.buffer.len()
    }
}

fn calculate_volume_scaled(volume: f32) -> f32 {
    // Above unity the slider is no longer a taper: 1.0–1.5 is straight linear
    // gain, so 150% is 1.5x amplitude (+3.5 dB) and the readout does not lie
    // about how much louder it gets.
    if volume > 1.0 {
        return volume;
    }

    let volume = volume as f64;

    let result = {
        if volume >= 0.99 {
            1.0
        } else if volume > 0.1 {
            f64::exp(3.91202300543 * volume) / 50.0
        } else {
            volume * 0.295751527165
        }
    };
    result as f32
}

#[cfg(test)]
mod tests {
    use cpal::traits::HostTrait;

    fn make_test_device() -> (Arc<cpal::Host>, Arc<cpal::Device>) {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| AudioError::DeviceNotFound("default".to_string()))
            .unwrap();

        (Arc::new(host), Arc::new(device))
    }

    fn make_test_config() -> OutputConfig {
        OutputConfig {
            sample_rate: 44100,
            channels: 2,
            bit_depth: 24,
        }
    }

    fn make_test_buffer() -> Arc<AudioRingBuffer> {
        Arc::new(AudioRingBuffer::new(100 * 2 * 44100 * 24))
    }

    use super::*;
    use audio_common::{AudioSamples, ChannelCount, Metadata};
    use std::thread;

    fn make_test_batch(samples: Vec<f32>) -> AudioBatch {
        AudioBatch {
            data: AudioSamples::F32(samples),
            metadata: Metadata {
                sample_rate: 44100,
                channels: ChannelCount::Stereo,
                bit_depth: 32,
            },
        }
    }

    #[test]
    fn test_default_output() {
        let (h, d) = make_test_device();
        let selected_device = SelectedOutputDevice { host: h, device: d };
        let output = CpalOutputStream::new(make_test_buffer(), make_test_config(), selected_device);
        assert!(output.is_ok(), "Should create default output");
    }

    #[test]
    fn test_device_lost_flag_is_edge_triggered() {
        let (h, d) = make_test_device();
        let selected_device = SelectedOutputDevice { host: h, device: d };
        let output =
            CpalOutputStream::new(make_test_buffer(), make_test_config(), selected_device).unwrap();

        assert!(!output.take_device_lost());

        output.device_lost.store(true, Ordering::SeqCst);
        assert!(output.take_device_lost());
        assert!(!output.take_device_lost());
    }

    #[test]
    fn test_buffer_underrun_does_not_mark_device_lost() {
        let flag = AtomicBool::new(false);
        on_stream_error(cpal::StreamError::BufferUnderrun, &flag);
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_device_not_available_marks_device_lost() {
        let flag = AtomicBool::new(false);
        on_stream_error(cpal::StreamError::DeviceNotAvailable, &flag);
        assert!(flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_stream_invalidated_marks_device_lost() {
        let flag = AtomicBool::new(false);
        on_stream_error(cpal::StreamError::StreamInvalidated, &flag);
        assert!(flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_pause_resume() {
        let (h, d) = make_test_device();
        let selected_device = SelectedOutputDevice { host: h, device: d };
        let output =
            CpalOutputStream::new(make_test_buffer(), make_test_config(), selected_device).unwrap();
        output.resume();
        assert!(output.is_playing());

        output.pause();
        thread::sleep(std::time::Duration::from_millis(10));
        assert!(!output.is_playing());

        output.resume();
        thread::sleep(std::time::Duration::from_millis(10));
        assert!(output.is_playing());
    }

    #[test]
    fn test_write_when_paused() {
        let (h, d) = make_test_device();
        let selected_device = SelectedOutputDevice { host: h, device: d };
        let output =
            CpalOutputStream::new(make_test_buffer(), make_test_config(), selected_device).unwrap();
        output.pause();
        thread::sleep(std::time::Duration::from_millis(10));

        output.write(&make_test_batch(vec![0.5, -0.5]));

        let buf = &output.buffer;
        assert!(buf.is_empty(), "Should not write when paused");
    }

    #[test]
    fn test_clear() {
        let (h, d) = make_test_device();
        let selected_device = SelectedOutputDevice { host: h, device: d };
        let output =
            CpalOutputStream::new(make_test_buffer(), make_test_config(), selected_device).unwrap();
        output.resume();
        output.write(&make_test_batch(vec![0.5, -0.5, 0.3, -0.3]));

        let buf = &output.buffer;
        assert_eq!(buf.len(), 4);

        output.clear();

        assert!(buf.is_empty());
    }

    #[test]
    fn test_pause_silences_and_release_keeps_it_paused() {
        let (h, d) = make_test_device();
        let selected_device = SelectedOutputDevice { host: h, device: d };
        let output =
            CpalOutputStream::new(make_test_buffer(), make_test_config(), selected_device).unwrap();
        assert!(output.silenced.load(Ordering::SeqCst));

        output.resume();
        assert!(!output.silenced.load(Ordering::SeqCst));

        output.pause();
        assert!(output.silenced.load(Ordering::SeqCst));
        assert!(!output.is_playing());

        output.release();
        assert!(output.silenced.load(Ordering::SeqCst));
        assert!(!output.is_playing());

        output.resume();
        assert!(!output.silenced.load(Ordering::SeqCst));
        assert!(output.is_playing());
    }

    #[test]
    fn test_release_leaves_playing_stream_alone() {
        let (h, d) = make_test_device();
        let selected_device = SelectedOutputDevice { host: h, device: d };
        let output =
            CpalOutputStream::new(make_test_buffer(), make_test_config(), selected_device).unwrap();
        output.resume();
        output.release();
        assert!(output.is_playing());
        assert!(!output.silenced.load(Ordering::SeqCst));
    }

    #[test]
    fn test_silenced_callback_keeps_buffer() {
        let fade = FadeState::new();
        let silenced = AtomicBool::new(true);
        let buffer = AudioRingBuffer::new(64);
        buffer.push_slice(&[0.5; 8]);
        let volume = AtomicF32::new(1.0);
        let track_gain = AtomicF32::new(1.0);
        let limiter = LimiterState::new(44100);
        let mut data = [1.0f32; 4];

        let gains = Gains {
            volume: &volume,
            track_gain: &track_gain,
            limiter: &limiter,
        };
        fill_f32(&fade, &silenced, &buffer, gains, 2, &mut data);
        assert_eq!(data, [0.0; 4]);
        assert_eq!(buffer.len(), 8);

        silenced.store(false, Ordering::SeqCst);
        let gains = Gains {
            volume: &volume,
            track_gain: &track_gain,
            limiter: &limiter,
        };
        fill_f32(&fade, &silenced, &buffer, gains, 2, &mut data);
        assert_eq!(data, [0.5; 4]);
        assert_eq!(buffer.len(), 4);
    }

    #[test]
    fn test_convert_s16_to_f32() {
        let samples = AudioSamples::S16(vec![0, 32767, -32768, 0]);
        let f32_samples = samples.to_f32();

        assert_eq!(f32_samples.len(), 4);
        assert!((f32_samples[0] - 0.0).abs() < 0.001);
        assert!((f32_samples[1] - 1.0).abs() < 0.001);
        assert!((f32_samples[2] - (-1.0)).abs() < 0.001);
        assert!((f32_samples[3] - 0.0).abs() < 0.001);
    }

    #[test]
    fn volume_above_unity_is_linear_gain() {
        assert_eq!(calculate_volume_scaled(1.0), 1.0);
        assert_eq!(calculate_volume_scaled(1.25), 1.25);
        assert_eq!(calculate_volume_scaled(MAX_VOLUME), MAX_VOLUME);
        // The taper below 100% is untouched.
        assert!(calculate_volume_scaled(0.5) < 0.5);
    }

    #[test]
    fn limiter_pulls_an_over_hot_frame_to_full_scale() {
        let limiter = LimiterState::new(44100);
        // 1.5x boost on a mastered-at-0.9 signal: 1.35 before the limiter.
        let mut data = [0.9f32 * MAX_VOLUME, -0.9 * MAX_VOLUME];
        apply_limiter(&mut data, 1, &limiter);
        for sample in data {
            assert!(sample.abs() <= 1.0 + 1e-5, "{sample} is above unity");
        }
        // Both channels share the gain, so the image does not wobble.
        assert!((data[0] + data[1]).abs() < 1e-6);
    }

    #[test]
    fn limiter_leaves_quiet_audio_alone() {
        let limiter = LimiterState::new(44100);
        let mut data = [0.25f32, -0.5];
        apply_limiter(&mut data, 2, &limiter);
        assert_eq!(data, [0.25, -0.5]);
    }

    #[test]
    fn limiter_recovers_toward_unity_after_a_loud_frame() {
        let limiter = LimiterState::new(44100);
        let mut loud = [2.0f32, -2.0];
        apply_limiter(&mut loud, 2, &limiter);
        let attenuated = limiter.gain.load(Ordering::Relaxed);
        assert!(attenuated < 0.6);

        // A second of quiet frames is enough for a 150 ms release to finish.
        let mut quiet = vec![0.1f32; 44100];
        apply_limiter(&mut quiet, 1, &limiter);
        assert!((limiter.gain.load(Ordering::Relaxed) - 1.0).abs() < 0.01);
        // The very first quiet frame still carries the old gain; the end of
        // the second has recovered.
        assert!((quiet[0] - 0.05).abs() < 0.01);
        assert!((quiet[44099] - 0.1).abs() < 0.01);
    }

    #[test]
    fn boost_is_applied_and_limited_end_to_end() {
        let fade = FadeState::new();
        let silenced = AtomicBool::new(false);
        let buffer = AudioRingBuffer::new(64);
        buffer.push_slice(&[0.9, -0.9, 0.01, -0.01]);
        // The callback stores the scaled value, exactly like `set_volume`.
        let volume = AtomicF32::new(calculate_volume_scaled(MAX_VOLUME));
        let track_gain = AtomicF32::new(1.0);
        let limiter = LimiterState::new(44100);
        let mut data = [0.0f32; 4];

        let gains = Gains {
            volume: &volume,
            track_gain: &track_gain,
            limiter: &limiter,
        };
        fill_f32(&fade, &silenced, &buffer, gains, 2, &mut data);
        // Boosted, then held under full scale by the limiter.
        assert!(data.iter().all(|sample| sample.abs() <= 1.0 + 1e-5));
        assert!(data[0] > 0.9, "the boost should still raise a loud track");
        // The quiet frame right after is attenuated by the release, not zeroed.
        assert!(data[2] > 0.0);
    }
}
