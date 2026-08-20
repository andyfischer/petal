//! Audio transport and mixing: the device handle, the PCM sound bank, and the
//! once-per-frame pump that keeps the queue fed.
//!
//! `petal-sdl` gives us a queued (not callback) device precisely so synthesis
//! can happen on the main thread inside our own frame — which is what makes
//! Petal-authored DSP possible at all. The cost is that nobody tops the queue
//! up for us: [`AudioEngine::pump`] must run every frame, in every run mode,
//! and must keep a lead of a few frames so one long frame does not underrun.
//!
//! **Audio is never fatal.** A machine with no sound device, and every run mode
//! that does not init SDL (headless, screenshot, record — see
//! `Host::on_sdl_init`), leave [`AudioEngine::out`] as `None`. Everything here
//! still runs and still advances the APU's state; the samples are simply
//! dropped. A cart must behave identically with and without a device.
//!
//! STUB: opens the device and pumps chip output (which is currently silence).
//! The sound bank, voice mixing, and the realtime Petal DSP bus are the audio
//! task's work.

use std::collections::HashMap;

use sdl2::Sdl;

use petal_sdl::AudioOutput;

use crate::apu::Apu;

/// Requested device format. `AudioOutput` reports what was actually obtained;
/// synthesis must use [`AudioEngine::sample_rate`], not this.
pub const SAMPLE_RATE: i32 = 44_100;
pub const CHANNELS: u8 = 2;
/// SDL's internal device buffer, in frames. ~11ms at 44.1kHz — short enough
/// that a sound effect feels attached to the frame that triggered it.
pub const DEVICE_BUFFER_FRAMES: u16 = 512;

/// How many frames of audio to keep queued ahead of the device. Three frames
/// of slack absorbs an occasional long frame without adding audible latency.
pub const TARGET_LEAD_FRAMES: u32 = 3;

/// Nominal video frame rate, used to size one pump's worth of samples.
pub const FRAME_RATE: u32 = 60;

/// A decoded sound in the bank: mono samples in -1..1 at the device rate.
pub struct Sound {
    pub samples: Vec<f32>,
}

/// One playing instance of a banked sound.
struct Voice {
    name: String,
    cursor: usize,
    volume: f32,
}

pub struct AudioEngine {
    /// `None` when no device could be opened, or when this run mode never
    /// initialized SDL. Everything degrades to "render and discard".
    out: Option<AudioOutput>,
    bank: HashMap<String, Sound>,
    voices: Vec<Voice>,
    /// Scratch mix buffer, reused across frames so the pump does not allocate.
    scratch: Vec<i16>,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEngine {
    pub fn new() -> Self {
        Self {
            out: None,
            bank: HashMap::new(),
            voices: Vec::new(),
            scratch: Vec::new(),
        }
    }

    /// Open the playback device. Called from `Host::on_sdl_init`, so only in
    /// the windowed run modes. A failure is reported once and then ignored.
    pub fn open(&mut self, sdl: &Sdl) {
        match AudioOutput::open(sdl, SAMPLE_RATE, CHANNELS, DEVICE_BUFFER_FRAMES) {
            Ok(out) => {
                out.resume();
                self.out = Some(out);
            }
            Err(e) => eprintln!("[fantasy-nes audio] no sound device: {}", e),
        }
    }

    pub fn is_open(&self) -> bool {
        self.out.is_some()
    }

    /// The rate synthesis must use — what the device actually gave us, falling
    /// back to the requested rate when there is no device.
    pub fn sample_rate(&self) -> u32 {
        self.out
            .as_ref()
            .map(|o| o.sample_rate() as u32)
            .unwrap_or(SAMPLE_RATE as u32)
    }

    pub fn channels(&self) -> u8 {
        self.out.as_ref().map(|o| o.channels()).unwrap_or(CHANNELS)
    }

    /// Install (or replace) a rendered sound. Replacing is the hot-reload path:
    /// a cart that changes its synthesis function re-registers under the same
    /// name and playing voices pick up the new samples.
    pub fn register_sound(&mut self, name: &str, samples: Vec<f32>) {
        self.bank.insert(name.to_string(), Sound { samples });
    }

    pub fn has_sound(&self, name: &str) -> bool {
        self.bank.contains_key(name)
    }

    /// Start a voice. Unknown names are ignored: a cart may call `play_sound`
    /// before the frame that registers it, and warning every frame would be
    /// worse than the silence.
    pub fn play_sound(&mut self, name: &str, volume: f32) {
        if !self.bank.contains_key(name) {
            return;
        }
        self.voices.push(Voice {
            name: name.to_string(),
            cursor: 0,
            volume: volume.clamp(0.0, 1.0),
        });
    }

    /// Stop every voice of a sound.
    pub fn stop_sound(&mut self, name: &str) {
        self.voices.retain(|v| v.name != name);
    }

    /// Drop every playing voice and discard queued audio — a hard cut, for
    /// switching carts.
    pub fn reset(&mut self) {
        self.voices.clear();
        if let Some(out) = &self.out {
            out.clear();
        }
    }

    /// Top the device queue up to [`TARGET_LEAD_FRAMES`] frames of lead.
    /// Called once per frame from `Host::end_frame`, in every run mode.
    ///
    /// The APU is advanced even with no device so that a headless run and a
    /// windowed run see the same channel state; only the queueing is skipped.
    pub fn pump(&mut self, apu: &mut Apu) {
        let rate = self.sample_rate();
        let channels = self.channels() as usize;
        let frame_samples = (rate / FRAME_RATE) as usize;

        let lead = self.out.as_ref().map(|o| o.queued_frames()).unwrap_or(0);
        let want_frames = if lead >= TARGET_LEAD_FRAMES * frame_samples as u32 {
            0
        } else {
            frame_samples
        };
        if want_frames == 0 {
            return;
        }

        self.scratch.clear();
        self.scratch.resize(want_frames * channels, 0);
        apu.render(&mut self.scratch, rate);
        // STUB: the PCM voices in `self.voices` are not mixed in yet.

        if let Some(out) = &self.out
            && let Err(e) = out.queue_samples(&self.scratch)
        {
            eprintln!("[fantasy-nes audio] {}", e);
        }
    }
}
