//! Audio transport: a thin safe wrapper over SDL's queued-audio device.
//!
//! This module is deliberately dumb. It knows how to open a playback device,
//! push interleaved 16-bit samples at it, and report how much is still buffered
//! — nothing about synthesis, mixing, notes, or instruments. Hosts own their own
//! sound generation and use this only to get the result out of the process.
//!
//! We use `AudioQueue` (SDL's pull-free path) rather than an `AudioCallback`
//! because Petal's `Env` is single-threaded and lives on the main thread: a
//! callback runs on SDL's audio thread and could not call into a script. With a
//! queue the host synthesizes during its own frame — typically from
//! [`crate::game_loop::Host::end_frame`] — and tops the device up, keeping a
//! lead of a few frames' worth of samples so an occasional long frame does not
//! underrun. [`AudioOutput::queued_frames`] is what that decision is made on.
//!
//! Everything degrades to an error `Result` rather than a panic: a machine with
//! no sound device should still run the app.

use sdl2::Sdl;
use sdl2::audio::{AudioQueue, AudioSpecDesired};

/// An open playback device fed by pushing samples at it.
///
/// Samples are interleaved `i16` — for stereo, `[l0, r0, l1, r1, ...]`. A
/// *frame* is one sample per channel, which is the unit every count here is
/// reported in (SDL counts bytes; that conversion is this type's job).
pub struct AudioOutput {
    queue: AudioQueue<i16>,
    sample_rate: i32,
    channels: u8,
}

impl AudioOutput {
    /// Open the default playback device. `buffer_frames` is SDL's internal
    /// device buffer (rounded up to a power of two by SDL); it bounds latency,
    /// not how much this wrapper will hold.
    ///
    /// The device starts **paused** — call [`AudioOutput::resume`] once the
    /// host has queued its first block, so playback does not begin on silence.
    ///
    /// The device may hand back a rate or channel count different from the one
    /// requested; [`AudioOutput::sample_rate`] and [`AudioOutput::channels`]
    /// report what was actually obtained, and callers should synthesize against
    /// those rather than the values they asked for.
    pub fn open(
        sdl: &Sdl,
        sample_rate: i32,
        channels: u8,
        buffer_frames: u16,
    ) -> Result<Self, String> {
        let audio = sdl.audio()?;
        let desired = AudioSpecDesired {
            freq: Some(sample_rate),
            channels: Some(channels),
            samples: Some(buffer_frames),
        };
        let queue: AudioQueue<i16> = audio.open_queue(None, &desired)?;
        let spec = queue.spec();
        let (sample_rate, channels) = (spec.freq, spec.channels);
        Ok(Self {
            queue,
            sample_rate,
            channels,
        })
    }

    /// Samples per second per channel, as obtained from the device.
    pub fn sample_rate(&self) -> i32 {
        self.sample_rate
    }

    /// Channel count, as obtained from the device.
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// Append interleaved samples to the device queue. A slice whose length is
    /// not a whole number of frames is rejected rather than silently shifting
    /// the channel interleave for everything queued after it.
    pub fn queue_samples(&self, interleaved: &[i16]) -> Result<(), String> {
        if !interleaved.len().is_multiple_of(self.channels as usize) {
            return Err(format!(
                "audio: {} samples is not a whole number of {}-channel frames",
                interleaved.len(),
                self.channels
            ));
        }
        self.queue.queue_audio(interleaved)
    }

    /// How many frames are still waiting to be played. The host's cue for how
    /// much to synthesize this frame: top up toward a target lead, and skip
    /// when already ahead.
    pub fn queued_frames(&self) -> u32 {
        let bytes_per_frame = size_of::<i16>() as u32 * self.channels as u32;
        self.queue.size() / bytes_per_frame
    }

    /// Start (or restart) playback.
    pub fn resume(&self) {
        self.queue.resume();
    }

    /// Stop playback without discarding what is queued.
    pub fn pause(&self) {
        self.queue.pause();
    }

    /// Discard everything queued — for a hard cut, e.g. switching carts or
    /// recovering from a stall that built up a long backlog.
    pub fn clear(&self) {
        self.queue.clear();
    }
}
