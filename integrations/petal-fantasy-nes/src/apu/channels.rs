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

#[derive(Clone, Copy, Debug, Default)]
pub struct Pulse {
    /// Normalized phase in 0..1.
    pub phase: f32,
    pub hz: f32,
    pub duty: u8,
    pub volume: u8,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Triangle {
    pub phase: f32,
    pub hz: f32,
    pub on: bool,
}

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
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            lfsr: 1,
            accum: 0.0,
            period: 0,
            volume: 0,
            mode: 0,
        }
    }
}
