//! Number and string formatting: `fixed`, `commas`, `pad_start`/`pad_end`, and
//! the printf-style `format`.
//!
//! These exist because `str(x)` prints a float the way Rust's `{}` does —
//! `str(12.3 * 3)` is `"36.900000000000006"` — which is correct and unusable in
//! a table, a dashboard, or a price. Every non-trivial script was hand-rolling
//! the same two helpers (round-and-pad decimals, insert thousands separators)
//! before this module existed.
//!
//! ## Why `%` and not `{}`
//!
//! The lexer gives `{` a meaning inside string literals (interpolation), so a
//! `"{:.2f}"` template cannot even be written as a literal. The placeholders
//! here are therefore printf-style and start with `%`, which the lexer has no
//! opinion about. No lexer change is involved in any of this.

use crate::native_fn::PetalCxt;
use crate::value::{self, Value};

/// Render `x` with exactly `places` digits after the decimal point.
/// `places` is clamped to 0..=17 — beyond an f64's precision the extra digits
/// are noise, and `format!` would happily print hundreds of them.
fn fixed_string(x: f64, places: i64) -> String {
    let places = places.clamp(0, 17) as usize;
    // Round explicitly before formatting, because Rust's `{:.N}` breaks an exact
    // tie to *even* (`format!("{:.1}", 91.25)` is `"91.2"`) while `round(x, N)`
    // rounds half away from zero (`91.3`). Two builtins that disagree about a
    // displayed digit is a bug report waiting to happen, so `fixed` follows
    // `round`. The guard keeps the scaling from overflowing to infinity on a
    // huge value, where every digit shown is above the rounding position anyway.
    let scaled = x * 10f64.powi(places as i32);
    let x = if x.is_finite() && scaled.abs() < 1e15 {
        scaled.round() / 10f64.powi(places as i32)
    } else {
        x
    };
    let s = format!("{:.*}", places, x);
    // `-0.00` is an artifact of a tiny negative input, never something a caller
    // means to display.
    if s.starts_with('-') && s[1..].chars().all(|c| c == '0' || c == '.') {
        s[1..].to_string()
    } else {
        s
    }
}

/// Insert `,` every three digits into the integer part of an already-rendered
/// number. Everything from the first `.` on (and a leading sign) is preserved
/// untouched, so this composes with [`fixed_string`].
fn group_thousands(s: &str) -> String {
    let (sign, rest) = match s.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", s),
    };
    let (int_part, frac_part) = match rest.find('.') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    // Anything that is not a run of digits (`inf`, `NaN`, an exponent form) has
    // no thousands to group; hand it back unchanged rather than mangling it.
    if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return s.to_string();
    }
    let mut grouped = String::with_capacity(int_part.len() + int_part.len() / 3);
    for (i, c) in int_part.chars().enumerate() {
        if i > 0 && (int_part.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{sign}{grouped}{frac_part}")
}

/// Coerce a formatting argument to f64. Accepts the numeric values and the
/// dual-number primal; anything else is a caller error worth naming.
fn as_number(v: Value, who: &str) -> Result<f64, String> {
    match v {
        Value::Int(n) => Ok(n as f64),
        Value::Float(f) => Ok(f),
        Value::Dual { value, .. } => Ok(value),
        Value::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
        other => Err(format!(
            "{}() expects a number, got {}",
            who,
            other.type_name()
        )),
    }
}

/// `fixed(x, places)` → `x` as a string with exactly `places` decimals.
/// `fixed(12.3456, 2)` is `"12.35"`; `fixed(5, 2)` is `"5.00"`. `places`
/// defaults to 2 when omitted.
pub(super) fn native_fixed(state: &mut PetalCxt) -> Result<u32, String> {
    let places = match state.arg_count() {
        1 => 2,
        2 => state.get_int(2)?,
        n => {
            return Err(format!(
                "fixed() expects 1 or 2 arguments (value, places), got {n}"
            ));
        }
    };
    let x = as_number(state.get_value(1)?, "fixed")?;
    let s = fixed_string(x, places);
    state.push_string(s);
    Ok(1)
}

/// `commas(n)` → `n` as a thousands-separated string: `commas(1234567)` is
/// `"1,234,567"`. With a second argument it rounds to that many decimals first,
/// so `commas(1234.5678, 2)` is `"1,234.57"`; without one an integer prints as
/// an integer and a float keeps its natural rendering.
pub(super) fn native_commas(state: &mut PetalCxt) -> Result<u32, String> {
    let argc = state.arg_count();
    if argc != 1 && argc != 2 {
        return Err(format!(
            "commas() expects 1 or 2 arguments (value, places), got {argc}"
        ));
    }
    let v = state.get_value(1)?;
    let rendered = if argc == 2 {
        let places = state.get_int(2)?;
        fixed_string(as_number(v, "commas")?, places)
    } else {
        match v {
            // An int has no fractional part to preserve and must never gain one.
            Value::Int(n) => n.to_string(),
            other => {
                // Reuse the display rendering so `commas(x)` and `str(x)` never
                // disagree about the digits, only about the separators.
                as_number(other, "commas")?;
                value::value_to_display_string(&other, state.heap())
            }
        }
    };
    let s = group_thousands(&rendered);
    state.push_string(s);
    Ok(1)
}

/// Pad `s` to `width` characters (not bytes) with `fill`, on the given side.
fn pad(s: &str, width: i64, fill: &str, at_start: bool) -> String {
    let fill_char = fill.chars().next().unwrap_or(' ');
    let len = s.chars().count() as i64;
    if len >= width {
        return s.to_string();
    }
    let padding: String = std::iter::repeat(fill_char)
        .take((width - len) as usize)
        .collect();
    if at_start {
        format!("{padding}{s}")
    } else {
        format!("{s}{padding}")
    }
}

/// The shared body of `pad_start`/`pad_end`: `(value, width, fill = " ")`.
/// A non-string value is rendered with `str()` semantics first, so
/// `pad_start(7, 3, "0")` is `"007"` without a cast at the call site.
fn native_pad(state: &mut PetalCxt, name: &str, at_start: bool) -> Result<u32, String> {
    let argc = state.arg_count();
    if argc != 2 && argc != 3 {
        return Err(format!(
            "{name}() expects 2 or 3 arguments (value, width, fill), got {argc}"
        ));
    }
    let v = state.get_value(1)?;
    let width = state.get_int(2)?;
    let fill = if argc == 3 {
        state.get_string(3)?
    } else {
        " ".to_string()
    };
    let s = value::value_to_display_string(&v, state.heap());
    let padded = pad(&s, width, &fill, at_start);
    state.push_string(padded);
    Ok(1)
}

pub(super) fn native_pad_start(state: &mut PetalCxt) -> Result<u32, String> {
    native_pad(state, "pad_start", true)
}

pub(super) fn native_pad_end(state: &mut PetalCxt) -> Result<u32, String> {
    native_pad(state, "pad_end", false)
}

/// One parsed `%` placeholder.
struct Spec {
    /// Group the integer part with thousands separators (`%,d`, `%,.2f`).
    group: bool,
    /// Digits after the decimal point (`.N`), when given.
    precision: Option<i64>,
    /// Minimum field width (`%8s`), padded on the left, or on the right when
    /// the width was written negative (`%-8s`).
    width: Option<i64>,
    left_align: bool,
    /// The conversion letter.
    kind: char,
}

/// Parse the placeholder body that follows a `%`, returning it and the number of
/// bytes consumed. Grammar: `%[-][,][width][.precision]kind`.
fn parse_spec(chars: &[char]) -> Result<(Spec, usize), String> {
    let mut i = 0;
    let mut spec = Spec {
        group: false,
        precision: None,
        width: None,
        left_align: false,
        kind: 's',
    };
    if chars.get(i) == Some(&'-') {
        spec.left_align = true;
        i += 1;
    }
    if chars.get(i) == Some(&',') {
        spec.group = true;
        i += 1;
    }
    let start = i;
    while chars.get(i).is_some_and(|c| c.is_ascii_digit()) {
        i += 1;
    }
    if i > start {
        let w: String = chars[start..i].iter().collect();
        spec.width = w.parse::<i64>().ok();
    }
    if chars.get(i) == Some(&'.') {
        i += 1;
        let pstart = i;
        while chars.get(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
        }
        let p: String = chars[pstart..i].iter().collect();
        spec.precision = Some(p.parse::<i64>().unwrap_or(0));
    }
    match chars.get(i) {
        Some(&c) if "sdfx%".contains(c) => {
            spec.kind = c;
            i += 1;
            Ok((spec, i))
        }
        Some(&c) => Err(format!(
            "format(): unknown conversion '%{c}' — expected one of %s %d %f %x %%"
        )),
        None => Err("format(): template ends with a dangling '%'".into()),
    }
}

/// `format(template, args...)` — printf-style templating with `%` placeholders.
///
/// | placeholder | meaning                                        |
/// |-------------|------------------------------------------------|
/// | `%s`        | the value, rendered as `str()` would           |
/// | `%d`        | the value as a whole number (truncated)        |
/// | `%f`        | the value as a float; `%.2f` fixes 2 decimals  |
/// | `%x`        | the value in lowercase hex                     |
/// | `%%`        | a literal `%`                                  |
///
/// A `,` right after the `%` groups thousands (`%,d`, `%,.2f`). A number after
/// it sets a minimum field width, padded on the left, or on the right if the
/// width is preceded by `-`: `%-10s`, `%8.2f`.
///
/// Deliberately `%`-based rather than `{}`-based: `{` already means
/// interpolation inside a Petal string literal, so a `{}` template could not be
/// written as one.
pub(super) fn native_format(state: &mut PetalCxt) -> Result<u32, String> {
    if state.arg_count() == 0 {
        return Err("format() expects at least 1 argument (template)".into());
    }
    let template = state.get_string(1)?;
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::with_capacity(template.len());
    let mut next_arg = 2usize;
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] != '%' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let (spec, used) = parse_spec(&chars[i + 1..])?;
        i += 1 + used;
        if spec.kind == '%' {
            out.push('%');
            continue;
        }
        if next_arg > state.arg_count() {
            return Err(format!(
                "format(): template needs more arguments than the {} given",
                state.arg_count() - 1
            ));
        }
        let v = state.get_value(next_arg)?;
        next_arg += 1;
        let mut rendered = match spec.kind {
            's' => match spec.precision {
                // `%.2s` on a number is the same rounding `fixed` does; on a
                // string it would be a truncation nobody asked for, so it only
                // applies to numbers.
                Some(p) if !matches!(v, Value::String(_)) => {
                    fixed_string(as_number(v, "format")?, p)
                }
                _ => value::value_to_display_string(&v, state.heap()),
            },
            'd' => {
                let n = as_number(v, "format")?;
                // Truncate toward zero, matching `int(x)`.
                format!("{}", n.trunc() as i64)
            }
            'f' => fixed_string(as_number(v, "format")?, spec.precision.unwrap_or(6)),
            'x' => format!("{:x}", as_number(v, "format")?.trunc() as i64),
            _ => unreachable!("parse_spec only accepts sdfx%"),
        };
        if spec.group {
            rendered = group_thousands(&rendered);
        }
        if let Some(w) = spec.width {
            rendered = pad(&rendered, w, " ", !spec.left_align);
        }
        out.push_str(&rendered);
    }
    state.push_string(out);
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_rounds_and_pads() {
        assert_eq!(fixed_string(12.3456, 2), "12.35");
        assert_eq!(fixed_string(5.0, 2), "5.00");
        assert_eq!(fixed_string(12.3, 0), "12");
        // The motivating case: float noise never reaches the reader.
        assert_eq!(fixed_string(12.3 * 3.0, 2), "36.90");
        // Exact ties round away from zero, matching `round(x, places)` rather
        // than Rust's ties-to-even `{:.1}` (which would say "91.2").
        assert_eq!(fixed_string(91.25, 1), "91.3");
        assert_eq!(fixed_string(-91.25, 1), "-91.3");
        // Huge values skip the pre-scaling and still render.
        assert_eq!(fixed_string(1e300, 0), format!("{:.0}", 1e300));
    }

    /// A tiny negative rounds to zero, and a displayed `-0.00` is never what the
    /// caller meant.
    #[test]
    fn negative_zero_is_plain_zero() {
        assert_eq!(fixed_string(-0.001, 2), "0.00");
        assert_eq!(fixed_string(-0.0, 1), "0.0");
        assert_eq!(fixed_string(-1.235, 2), "-1.24");
    }

    #[test]
    fn grouping_leaves_sign_and_fraction_alone() {
        assert_eq!(group_thousands("1234567"), "1,234,567");
        assert_eq!(group_thousands("123"), "123");
        assert_eq!(group_thousands("1000"), "1,000");
        assert_eq!(group_thousands("-1234567.25"), "-1,234,567.25");
        assert_eq!(group_thousands("0"), "0");
    }

    /// Nothing that isn't a digit run gets separators inserted into it.
    #[test]
    fn grouping_passes_through_non_numbers() {
        assert_eq!(group_thousands("inf"), "inf");
        assert_eq!(group_thousands("NaN"), "NaN");
        assert_eq!(group_thousands("1e21"), "1e21");
    }

    #[test]
    fn pad_counts_characters_not_bytes() {
        assert_eq!(pad("7", 3, "0", true), "007");
        assert_eq!(pad("ok", 4, " ", false), "ok  ");
        assert_eq!(pad("toolong", 3, " ", true), "toolong");
        assert_eq!(pad("é", 3, ".", true), "..é");
    }
}
