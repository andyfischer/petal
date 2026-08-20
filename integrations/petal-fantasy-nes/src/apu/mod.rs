//! The APU: two pulse channels, a triangle, and a noise channel.
//!
//! The model matches the PPU's: the cart writes channel state every frame and
//! the host renders a frame's worth of samples from whatever the state is now.
//! There is no sequencer, no envelope hardware, and no length counter — a cart
//! that wants a decaying note lowers the volume each frame, which is both
//! simpler to explain and strictly more expressive.
//!
//! Channel state is *sticky*: a note keeps sounding until the cart changes or
//! silences it. That is what makes [`Apu::mute`] necessary (a cart that stops
//! writing a channel is not asking for silence — it may simply be holding a
//! note), and it is why [`crate::audio::AudioEngine`] calls [`Apu::render`]
//! every frame whether or not the cart wrote anything.
//!
//! PCM playback is *not* here: sampled and Petal-synthesized sound live in
//! [`crate::audio`], which mixes them over this chip output.

pub mod channels;

use channels::{MAX_VOLUME, Noise, Pulse, Triangle, note_to_hz};

pub const PULSE_CHANNELS: usize = 2;

#[derive(Clone, Debug, Default)]
pub struct Apu {
    pub pulse: [Pulse; PULSE_CHANNELS],
    pub triangle: Triangle,
    pub noise: Noise,
}

impl Apu {
    pub fn new() -> Self {
        Self::default()
    }

    /// `ch` is 0 or 1, `note` is a (fractional) MIDI semitone, `duty` is 0-3,
    /// `volume` is 0-15 with 0 silencing the channel. Out-of-range arguments
    /// are clamped rather than rejected — a cart is untrusted input and a
    /// wrong note beats a killed frame.
    pub fn write_pulse(&mut self, ch: usize, note_semitones: f32, duty: u8, volume: u8) {
        if ch >= PULSE_CHANNELS {
            return;
        }
        let p = &mut self.pulse[ch];
        p.hz = note_to_hz(note_semitones);
        p.duty = duty & 3;
        p.volume = volume.min(MAX_VOLUME);
    }

    /// The triangle has no volume control on the hardware, so it is a gate:
    /// `on` false silences it, `on` true sounds it at full level.
    pub fn write_triangle(&mut self, note: f32, on: bool) {
        self.triangle.hz = note_to_hz(note);
        self.triangle.on = on;
    }

    /// `period` 0-15 selects one of the chip's noise rates (0 = highest),
    /// `mode` 0 is the long sequence and 1 the short metallic one.
    pub fn write_noise(&mut self, period: u8, volume: u8, mode: u8) {
        self.noise.period = period.min(15);
        self.noise.volume = volume.min(MAX_VOLUME);
        self.noise.mode = mode & 1;
    }

    /// Silence every channel, keeping phases so a resumed note does not click.
    /// Used on cart switch and on hot reload.
    pub fn mute(&mut self) {
        for p in self.pulse.iter_mut() {
            p.volume = 0;
        }
        self.triangle.on = false;
        self.noise.volume = 0;
    }

    /// Render into `out`, which is **interleaved stereo** — `[l, r, l, r, ...]`
    /// — so its length must be even. The chip is mono; both channels get the
    /// same signal, and the stereo layout exists only because that is what the
    /// device is opened with.
    ///
    /// STUB: writes silence. Waveform generation, mixing and level-matching
    /// are the APU task's work.
    pub fn render(&mut self, out: &mut [i16], _sample_rate: u32) {
        out.fill(0);
    }
}
