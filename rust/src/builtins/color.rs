//! Color builtins: hsv, hsl, color_lerp. All return RGB records
//! `{r, g, b}` with 0..255 integer channels, matching the shape produced
//! by color literals like `#ff8800`.

use crate::native_fn::PetalCxt;
use crate::value::Value;

use super::require_args;

/// HSV to RGB conversion. h: 0-360, s: 0-1, v: 0-1. Returns (r, g, b) 0-255.
fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (f64, f64, f64) {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 60.0 {
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
    };
    ((r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0)
}

/// HSL to RGB conversion. h: 0-360, s: 0-1, l: 0-1. Returns (r, g, b) 0-255.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = if h < 60.0 {
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
    };
    ((r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0)
}

fn push_color_map(state: &mut PetalCxt, r: f64, g: f64, b: f64) {
    let mut map = indexmap::IndexMap::new();
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
            let m1 = state.heap().get_map(id1);
            let r1 = m1.get("r").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let g1 = m1.get("g").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let b1 = m1.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let m2 = state.heap().get_map(id2);
            let r2 = m2.get("r").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let g2 = m2.get("g").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let b2 = m2.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let r = r1 + (r2 - r1) * t;
            let g = g1 + (g2 - g1) * t;
            let b = b1 + (b2 - b1) * t;
            push_color_map(state, r, g, b);
            Ok(1)
        }
        _ => Err("color_lerp() expects two color records {r, g, b}".into()),
    }
}
