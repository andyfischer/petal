//! The individual sound generators.
//!
//! Each channel is a phase accumulator plus whatever state its waveform needs.
//! Carts write *musical* parameters (a MIDI-ish note number, a duty index, a
//! 0-15 volume) and never touch periods or timer reloads: the point is the
//! sound of the chip, not its register map. Frequency is derived here, in one
//! place, so a pitch bend is just a fractional note.
//!
//! Every channel holds its phase across frames. A cart re-asserts a note every
//! frame; if that reset the phase, a held note would click at 60Hz.
//!
//! Two deliberate departures from the hardware, both because we render at
//! 44.1kHz rather than clocking a DAC at 1.79MHz:
//!
//! * **Band limiting.** A naively stepped square at 44.1kHz folds its whole
//!   harmonic series back down the spectrum, which sounds like a cheap buzzer
//!   rather than like a chip. The pulse channels place a polyBLEP correction
//!   at each edge, which removes most of that aliasing for the price of a few
//!   multiplies per sample. The triangle keeps its 32-step stair unsmoothed —
//!   those steps *are* the NES triangle's character, and at its rate they are
//!   quiet enough not to matter.
//! * **Anti-click ramps.** The cart changes volume and gate at 60Hz, i.e.
//!   between one sample and the next. Every channel therefore slews its output
//!   gain toward the written level over a couple of milliseconds
//!   ([`GAIN_SLEW_SECONDS`]) instead of jumping. The slew snaps to the target
//!   once it is close enough, so "volume 0" is eventually *exact* silence and
//!   not an inaudible-but-nonzero floor.

/// Concert-A reference for [`note_to_hz`]. Note 69 == 440Hz, the MIDI
/// convention, so cart-side note tables can be copied from anywhere.
pub const A4_NOTE: f32 = 69.0;
pub const A4_HZ: f32 = 440.0;

/// MIDI semitone number (fractional allowed) to frequency in Hz.
#[inline]
pub fn note_to_hz(note: f32) -> f32 {
    A4_HZ * ((note - A4_NOTE) / 12.0).exp2()
}

/// Duty cycles the pulse channels can select, as a fraction of the period the
/// wave spends high. These are the NES's four.
pub const DUTIES: [f32; 4] = [0.125, 0.25, 0.5, 0.75];

/// Volume levels are 0-15 as on the hardware; 0 is silence.
pub const MAX_VOLUME: u8 = 15;

/// Time constant of the per-channel gain slew. Long enough to swallow a step
/// from silence to full volume, short enough that a staccato note still starts
/// on the frame it was written.
pub const GAIN_SLEW_SECONDS: f32 = 0.002;

/// Below this distance the slew jumps to its target, so a settled channel is
/// bit-for-bit stable and a silenced one is exactly zero.
const GAIN_SNAP: f32 = 1.0e-4;

/// Highest frequency a channel will run at, as a fraction of the sample rate.
/// Past this the polyBLEP correction stops being meaningful and the waveform
/// is mostly alias anyway, so a cart writing a silly note gets a capped one
/// rather than noise.
const MAX_PHASE_INC: f32 = 0.45;

/// One pole of exponential smoothing per sample, for `seconds` of time
/// constant at `sample_rate`.
#[inline]
pub fn slew_coefficient(seconds: f32, sample_rate: f32) -> f32 {
    if seconds <= 0.0 || sample_rate <= 0.0 {
        return 1.0;
    }
    1.0 - (-1.0 / (seconds * sample_rate)).exp()
}

/// Move `gain` toward `target`, snapping when close. Returns the new gain.
#[inline]
fn slew(gain: f32, target: f32, coefficient: f32) -> f32 {
    let next = gain + (target - gain) * coefficient;
    if (target - next).abs() < GAIN_SNAP {
        target
    } else {
        next
    }
}

/// The polyBLEP residual for a unit step at normalized phase `t`, given a
/// phase increment of `dt` per sample. Adding it at a rising edge (and
/// subtracting it at the falling one) replaces the instantaneous jump with the
/// two-sample ramp a band-limited step would have produced.
#[inline]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// Advance a normalized phase and wrap it into 0..1.
#[inline]
fn advance(phase: f32, inc: f32) -> f32 {
    let p = phase + inc;
    if p >= 1.0 { p - p.floor() } else { p }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Pulse {
    /// Normalized phase in 0..1.
    pub phase: f32,
    pub hz: f32,
    pub duty: u8,
    pub volume: u8,
    /// Slewed output gain; see the module comment on anti-click ramps.
    pub gain: f32,
}

impl Pulse {
    /// One sample in -1..1. `sample_rate` is passed per call because the
    /// device rate is only known at runtime and can change on a cart switch.
    #[inline]
    pub fn next_sample(&mut self, sample_rate: f32, slew_coeff: f32) -> f32 {
        let target = self.volume.min(MAX_VOLUME) as f32 / MAX_VOLUME as f32;
        self.gain = slew(self.gain, target, slew_coeff);

        let inc = (self.hz.max(0.0) / sample_rate).min(MAX_PHASE_INC);
        // A stopped channel still has to hold its phase, and a silent one must
        // be exactly silent (the "volume 0" contract).
        if inc <= 0.0 || self.gain == 0.0 {
            return 0.0;
        }

        let duty = DUTIES[(self.duty & 3) as usize];
        let mut v = if self.phase < duty { 1.0 } else { -1.0 };
        v += poly_blep(self.phase, inc);
        let falling = if self.phase >= duty {
            self.phase - duty
        } else {
            self.phase - duty + 1.0
        };
        v -= poly_blep(falling, inc);

        self.phase = advance(self.phase, inc);
        v * self.gain
    }
}

/// Steps in the NES triangle's stair: 16 up, 16 down.
pub const TRIANGLE_STEPS: u32 = 32;

#[derive(Clone, Copy, Debug, Default)]
pub struct Triangle {
    pub phase: f32,
    pub hz: f32,
    pub on: bool,
    pub gain: f32,
}

impl Triangle {
    /// The stair, not a ramp: 16 discrete levels rising then falling, which is
    /// the whole reason the NES triangle sounds like a NES triangle rather
    /// than like an oscillator.
    #[inline]
    pub fn next_sample(&mut self, sample_rate: f32, slew_coeff: f32) -> f32 {
        let target = if self.on { 1.0 } else { 0.0 };
        self.gain = slew(self.gain, target, slew_coeff);

        let inc = (self.hz.max(0.0) / sample_rate).min(MAX_PHASE_INC);
        if inc <= 0.0 || self.gain == 0.0 {
            return 0.0;
        }

        let step = (self.phase * TRIANGLE_STEPS as f32) as u32 % TRIANGLE_STEPS;
        let level = if step < 16 { step } else { 31 - step } as f32;
        let v = (level - 7.5) / 7.5;

        self.phase = advance(self.phase, inc);
        v * self.gain
    }
}

/// NES noise timer periods in CPU cycles, index 0 the fastest. Divided into
/// [`NOISE_CLOCK_HZ`] these give the sixteen rates the hardware offers.
pub const NOISE_PERIODS: [u16; 16] = [
    4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
];

/// NTSC CPU clock, the noise timer's source.
pub const NOISE_CLOCK_HZ: f32 = 1_789_773.0;

/// Sequence lengths of the two LFSR taps. The short one is the metallic,
/// obviously-pitched mode.
pub const NOISE_LONG_PERIOD: u32 = 32767;
pub const NOISE_SHORT_PERIOD: u32 = 93;

#[derive(Clone, Copy, Debug)]
pub struct Noise {
    /// 15-bit linear-feedback shift register; never allowed to reach 0.
    pub lfsr: u16,
    /// Fractional counter of LFSR clocks owed at the current period.
    pub accum: f32,
    /// Period index 0-15 (0 = highest pitch).
    pub period: u8,
    pub volume: u8,
    /// 0 = long (32767-step) sequence, 1 = short (93-step) metallic sequence.
    pub mode: u8,
    pub gain: f32,
    /// Held output level between LFSR clocks.
    pub level: f32,
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            lfsr: 1,
            accum: 0.0,
            period: 0,
            volume: 0,
            mode: 0,
            gain: 0.0,
            level: 1.0,
        }
    }
}

impl Noise {
    /// Clock the shift register once. Bit 0 xor bit 1 (long) or bit 6 (short)
    /// feeds back into bit 14 — the hardware's tap layout, and the reason the
    /// short mode has a 93-step period.
    #[inline]
    pub fn clock(&mut self) {
        let tap = if self.mode & 1 == 1 { 6 } else { 1 };
        let feedback = (self.lfsr & 1) ^ ((self.lfsr >> tap) & 1);
        self.lfsr = (self.lfsr >> 1) | (feedback << 14);
        // The hardware mutes while bit 0 is set; as a bipolar generator that
        // is just the other half of the square.
        self.level = if self.lfsr & 1 == 0 { 1.0 } else { -1.0 };
    }

    #[inline]
    pub fn next_sample(&mut self, sample_rate: f32, slew_coeff: f32) -> f32 {
        let target = self.volume.min(MAX_VOLUME) as f32 / MAX_VOLUME as f32;
        self.gain = slew(self.gain, target, slew_coeff);

        // The register keeps running while muted: the noise channel is a
        // continuous hiss the cart gates, not a sound that restarts.
        let rate = NOISE_CLOCK_HZ / NOISE_PERIODS[(self.period & 15) as usize] as f32;
        self.accum += rate / sample_rate;
        while self.accum >= 1.0 {
            self.accum -= 1.0;
            self.clock();
        }

        if self.gain == 0.0 {
            return 0.0;
        }
        self.level * self.gain
    }
}

/// A one-shot sample player: the cart hands over a buffer of -1..1 mono
/// samples and this plays it once at the device rate. It exists on the chip
/// side (rather than only in the mixer) so that a cart's drum hit sits in the
/// same soft-clipped bus as the tonal channels.
#[derive(Clone, Debug, Default)]
pub struct Pcm {
    pub data: Vec<f32>,
    /// Playback position; `>= data.len()` means finished.
    pub cursor: usize,
    pub volume: f32,
    pub looping: bool,
}

impl Pcm {
    pub fn is_playing(&self) -> bool {
        !self.data.is_empty() && self.cursor < self.data.len()
    }

    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        if !self.is_playing() {
            return 0.0;
        }
        let v = self.data[self.cursor] * self.volume;
        self.cursor += 1;
        if self.looping && self.cursor >= self.data.len() {
            self.cursor = 0;
        }
        v
    }
}
