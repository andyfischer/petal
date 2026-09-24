//! Color builtins: hsv, hsl, color_lerp. All return RGB records
//! `{r, g, b}` with 0..255 integer channels, matching the shape produced
//! by color literals like `#ff8800`.

use crate::heap::MapId;
use crate::native_fn::PetalCxt;
use crate::value::Value;

use super::require_args;

/// Map a hue (degrees) and chroma to the un-lightened RGB sector `(r, g, b)`,
/// shared by the HSV and HSL conversions (which differ only in how they derive
/// chroma `c` and the lightness offset `m`).
fn hue_sector(h: f64, c: f64) -> (f64, f64, f64) {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    }
}

/// HSV to RGB conversion. h: 0-360, s: 0-1, v: 0-1. Returns (r, g, b) 0-255.
fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (f64, f64, f64) {
    let c = v * s;
    let m = v - c;
    let (r, g, b) = hue_sector(h, c);
    ((r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0)
}

/// HSL to RGB conversion. h: 0-360, s: 0-1, l: 0-1. Returns (r, g, b) 0-255.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let m = l - c / 2.0;
    let (r, g, b) = hue_sector(h, c);
    ((r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0)
}

fn push_color_map(state: &mut PetalCxt, r: f64, g: f64, b: f64) {
    let mut map = crate::heap::RecordMap::default();
    map.insert("r".to_string(), Value::Int(r.round() as i64));
    map.insert("g".to_string(), Value::Int(g.round() as i64));
    map.insert("b".to_string(), Value::Int(b.round() as i64));
    let map_id = state.heap_mut().alloc_map(map);
    state.push_value(Value::Map(map_id));
}

pub(super) fn native_hsv(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "hsv")?;
    // Hue is normalized to [0, 1) to match the rest of the color API (s, v,
    // alpha) and p5.js / three.js / Processing defaults. Use hsv_deg() for
    // degrees.
    let h = state.get_float(1)?;
    let s = state.get_float(2)?;
    let v = state.get_float(3)?;
    let (r, g, b) = hsv_to_rgb(h * 360.0, s, v);
    push_color_map(state, r, g, b);
    Ok(1)
}

pub(super) fn native_hsl(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "hsl")?;
    // Hue is normalized to [0, 1); use hsl_deg() for degrees.
    let h = state.get_float(1)?;
    let s = state.get_float(2)?;
    let l = state.get_float(3)?;
    let (r, g, b) = hsl_to_rgb(h * 360.0, s, l);
    push_color_map(state, r, g, b);
    Ok(1)
}

/// `hsv_deg(h, s, v)` — like `hsv` but with hue in degrees [0, 360).
pub(super) fn native_hsv_deg(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "hsv_deg")?;
    let h = state.get_float(1)?;
    let s = state.get_float(2)?;
    let v = state.get_float(3)?;
    let (r, g, b) = hsv_to_rgb(h, s, v);
    push_color_map(state, r, g, b);
    Ok(1)
}

/// `hsl_deg(h, s, l)` — like `hsl` but with hue in degrees [0, 360).
pub(super) fn native_hsl_deg(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "hsl_deg")?;
    let h = state.get_float(1)?;
    let s = state.get_float(2)?;
    let l = state.get_float(3)?;
    let (r, g, b) = hsl_to_rgb(h, s, l);
    push_color_map(state, r, g, b);
    Ok(1)
}

pub(super) fn native_color_lerp(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "color_lerp")?;
    let c1 = state.get_value(1)?;
    let c2 = state.get_value(2)?;
    let t = state.get_float(3)?;
    match (c1, c2) {
        (Value::Map(id1), Value::Map(id2)) => {
            lerp_color_maps(state, id1, id2, t);
            Ok(1)
        }
        _ => Err("color_lerp() expects two color records {r, g, b}".into()),
    }
}

/// Whether a record has the `r`, `g`, `b` number fields of a color.
pub(super) fn is_color_map(state: &PetalCxt, id: MapId) -> bool {
    let m = state.heap().get_map(id);
    ["r", "g", "b"]
        .iter()
        .all(|k| m.get(*k).and_then(|v| v.as_f64()).is_some())
}

/// Blend two color records channel by channel and push the result, rounded to
/// int channels like every other color. A missing `r`/`g`/`b` reads as 0. If
/// either side has an alpha `a`, so does the result, with a missing alpha
/// reading as opaque (255).
pub(super) fn lerp_color_maps(state: &mut PetalCxt, id1: MapId, id2: MapId, t: f64) {
    let channel = |state: &PetalCxt, id: MapId, k: &str| {
        state.heap().get_map(id).get(k).and_then(|v| v.as_f64())
    };
    let mix = |a: f64, b: f64| Value::Int((a + (b - a) * t).round() as i64);
    let mut map = crate::heap::RecordMap::default();
    for k in ["r", "g", "b"] {
        let a = channel(state, id1, k).unwrap_or(0.0);
        let b = channel(state, id2, k).unwrap_or(0.0);
        map.insert(k.to_string(), mix(a, b));
    }
    let (a1, a2) = (channel(state, id1, "a"), channel(state, id2, "a"));
    if a1.is_some() || a2.is_some() {
        map.insert("a".to_string(), mix(a1.unwrap_or(255.0), a2.unwrap_or(255.0)));
    }
    let map_id = state.heap_mut().alloc_map(map);
    state.push_value(Value::Map(map_id));
}
