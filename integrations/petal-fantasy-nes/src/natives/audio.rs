//! Sound natives: the chip channels, the PCM sound bank, and the realtime DSP
//! opt-in.
//!
//! Same buffered protocol as the video side ([`super`]): a native emits into
//! the `nes_audio` channel and [`apply`] walks it into the [`Apu`] and the
//! [`AudioEngine`] after the script has run.
//!
//! There is deliberately no `apply_for` — a speculative frame must not make
//! noise. Audio commands emitted on a fork are dropped along with the fork,
//! which is exactly the behavior an agent taking a screenshot wants.
//!
//! STUB: every native is registered and every call is emitted, but only the
//! chip writes are applied; `register_sound` / `enable_dsp` are accepted and
//! ignored. Sample rendering, the voice mixer, and the DSP budget are the audio
//! task's work.

use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::value::Value;

use crate::apu::Apu;
use crate::audio::AudioEngine;
use crate::natives::{Command, emit, opt_nums, take_commands};

/// Output channel carrying sound commands from the cart to the APU/mixer.
pub const AUDIO_CHANNEL: &str = "nes_audio";

pub fn register_audio(env: &mut Env) {
    // Chip channels
    env.register_native("apu_pulse", native_apu_pulse);
    env.register_native("apu_triangle", native_apu_triangle);
    env.register_native("apu_noise", native_apu_noise);
    env.register_native("apu_mute", native_apu_mute);

    // Petal-synthesized PCM
    env.register_native("register_sound", native_register_sound);
    env.register_native("play_sound", native_play_sound);
    env.register_native("stop_sound", native_stop_sound);
    env.register_native("enable_dsp", native_enable_dsp);
}

/// Apply this frame's sound commands from the live stack.
pub fn apply(env: &mut Env, apu: &mut Apu, engine: &mut AudioEngine) {
    let commands = take_commands(env, AUDIO_CHANNEL);
    for c in &commands {
        apply_command(c, apu, engine);
    }
}

fn apply_command(c: &Command, apu: &mut Apu, engine: &mut AudioEngine) {
    match c.tag.as_str() {
        "apu_pulse" => apu.write_pulse(c.usize(0), c.f32(1), c.u8(2), c.u8(3)),
        "apu_triangle" => apu.write_triangle(c.f32(0), c.bool(1)),
        "apu_noise" => apu.write_noise(c.u8(0), c.u8(1), c.u8(2)),
        "apu_mute" => apu.mute(),
        "play_sound" => {
            // The optional second argument defaults to full volume, so a bare
            // `play_sound(name)` is not silent.
            let volume = match c.args.len() {
                0 | 1 => 1.0,
                _ => c.f32(1),
            };
            engine.play_sound(c.str(0), volume);
        }
        "stop_sound" => engine.stop_sound(c.str(0)),
        // STUB: `register_sound` and `enable_dsp` need to call back into the
        // Env to run the cart's synthesis function, which is the audio task's
        // work. They are accepted and dropped for now.
        _ => {}
    }
}

// ── Chip channels ─────────────────────────────────────────────────────────

fn native_apu_pulse(cxt: &mut PetalCxt) -> NativeResult {
    let args = opt_nums(cxt, 4);
    emit(cxt, AUDIO_CHANNEL, "apu_pulse", args);
    cxt.push_nil();
    Ok(1)
}

fn native_apu_triangle(cxt: &mut PetalCxt) -> NativeResult {
    let args = opt_nums(cxt, 2);
    emit(cxt, AUDIO_CHANNEL, "apu_triangle", args);
    cxt.push_nil();
    Ok(1)
}

fn native_apu_noise(cxt: &mut PetalCxt) -> NativeResult {
    let args = opt_nums(cxt, 3);
    emit(cxt, AUDIO_CHANNEL, "apu_noise", args);
    cxt.push_nil();
    Ok(1)
}

fn native_apu_mute(cxt: &mut PetalCxt) -> NativeResult {
    emit(cxt, AUDIO_CHANNEL, "apu_mute", vec![]);
    cxt.push_nil();
    Ok(1)
}

// ── PCM / DSP ─────────────────────────────────────────────────────────────

fn native_register_sound(cxt: &mut PetalCxt) -> NativeResult {
    let name = string_arg(cxt, 1);
    let seconds = match cxt.get_value(2) {
        Ok(v @ (Value::Int(_) | Value::Float(_))) => v,
        _ => Value::Float(0.0),
    };
    let fn_name = string_arg(cxt, 3);
    emit(
        cxt,
        AUDIO_CHANNEL,
        "register_sound",
        vec![name, seconds, fn_name],
    );
    cxt.push_nil();
    Ok(1)
}

fn native_play_sound(cxt: &mut PetalCxt) -> NativeResult {
    let name = string_arg(cxt, 1);
    let mut args = vec![name];
    // Only forward a volume that was actually passed, so the apply site can
    // tell "no volume given" from "volume 0".
    if cxt.arg_count() >= 2 {
        args.push(match cxt.get_value(2) {
            Ok(v @ (Value::Int(_) | Value::Float(_))) => v,
            _ => Value::Float(1.0),
        });
    }
    emit(cxt, AUDIO_CHANNEL, "play_sound", args);
    cxt.push_nil();
    Ok(1)
}

fn native_stop_sound(cxt: &mut PetalCxt) -> NativeResult {
    let name = string_arg(cxt, 1);
    emit(cxt, AUDIO_CHANNEL, "stop_sound", vec![name]);
    cxt.push_nil();
    Ok(1)
}

fn native_enable_dsp(cxt: &mut PetalCxt) -> NativeResult {
    let fn_name = string_arg(cxt, 1);
    emit(cxt, AUDIO_CHANNEL, "enable_dsp", vec![fn_name]);
    cxt.push_nil();
    Ok(1)
}

/// Read a string argument into a fresh heap string, defaulting to empty.
fn string_arg(cxt: &mut PetalCxt, index: usize) -> Value {
    let s = cxt.get_string(index).unwrap_or_default();
    Value::String(cxt.heap_mut().alloc_string(s))
}
