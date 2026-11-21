//! Reading a document's output as a host draws it: element attributes with
//! defaults, colours, and the elements among an element's children.
//!
//! The output arrives as plain JSON ([`crate::Evaluated::outputs`],
//! [`crate::DocumentRun::plain_outputs`]): records are objects, and an element
//! is `{"type": "element", "tag", "props", "children"}`. [`Props`] reads the
//! props of one (or the fields of a record), each with the default the host
//! uses when the document leaves it out.
//!
//! ```ignore
//! let props = Props::of(&layer).unwrap_or_default();
//! let x = props.num("x", 0.0);
//! let fill = props.color("fill").unwrap_or([0, 0, 0, 255]);
//! ```

use serde_json::{Map, Value as Json};

/// A straight (not premultiplied) RGBA colour.
pub type Rgba = [u8; 4];

/// An element's props, or a record's fields, read with defaults. A field
/// set to `nil` reads as left out.
#[derive(Clone, Copy, Debug)]
pub struct Props<'a>(pub &'a Map<String, Json>);

impl Default for Props<'_> {
    fn default() -> Self {
        static EMPTY: std::sync::OnceLock<Map<String, Json>> = std::sync::OnceLock::new();
        Props(EMPTY.get_or_init(Map::new))
    }
}

impl<'a> Props<'a> {
    /// The props of an element, or the fields of a record; `None` for any
    /// other value.
    pub fn of(value: &'a Json) -> Option<Props<'a>> {
        let o = value.as_object()?;
        match o.get("type").and_then(Json::as_str) {
            Some("element") => o.get("props").and_then(Json::as_object).map(Props),
            _ => Some(Props(o)),
        }
    }

    pub fn get(&self, k: &str) -> Option<&'a Json> {
        self.0.get(k).filter(|v| !v.is_null())
    }
    pub fn has(&self, k: &str) -> bool {
        self.get(k).is_some()
    }
    /// A number, or `d` when it is left out, not a number, or not finite.
    pub fn num(&self, k: &str, d: f64) -> f64 {
        self.num_opt(k).unwrap_or(d)
    }
    pub fn num_opt(&self, k: &str) -> Option<f64> {
        self.get(k).and_then(Json::as_f64).filter(|v| v.is_finite())
    }
    pub fn str(&self, k: &str, d: &str) -> String {
        self.get(k).and_then(Json::as_str).unwrap_or(d).to_string()
    }
    pub fn bool(&self, k: &str, d: bool) -> bool {
        self.get(k).and_then(Json::as_bool).unwrap_or(d)
    }
    /// A colour (see [`color`]).
    pub fn color(&self, k: &str) -> Option<Rgba> {
        self.get(k).and_then(color)
    }
}

/// A colour value: a Petal colour (`:ff0066`, which arrives as `{r, g, b}`,
/// with `a` when it isn't opaque) or a hex string.
pub fn color(v: &Json) -> Option<Rgba> {
    match v {
        Json::Object(o) => {
            let c = |k: &str| o.get(k).and_then(Json::as_f64).map(|x| x.clamp(0.0, 255.0) as u8);
            Some([c("r")?, c("g")?, c("b")?, c("a").unwrap_or(255)])
        }
        Json::String(s) => parse_hex(s),
        _ => None,
    }
}

/// A hex colour as text: `#ff0066` as CSS writes it, `:ff0066` as Petal
/// does, or the digits alone; 3, 6 or 8 digits.
pub fn parse_hex(s: &str) -> Option<Rgba> {
    let s = s.trim();
    let h = s.strip_prefix(':').unwrap_or_else(|| s.trim_start_matches('#'));
    if !h.is_ascii() {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    match h.len() {
        6 => Some([p(0)?, p(2)?, p(4)?, 255]),
        8 => Some([p(0)?, p(2)?, p(4)?, p(6)?]),
        3 => {
            let d = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok().map(|v| v * 17);
            Some([d(0)?, d(1)?, d(2)?, 255])
        }
        _ => None,
    }
}

/// A list with its nested lists spliced in place and its `nil`s dropped: a
/// list a document builds with a `for` loop or an `if` without an `else`
/// among its entries, as a host draws it.
pub fn flatten(xs: &[Json]) -> Vec<Json> {
    let mut out = Vec::new();
    for x in xs {
        match x {
            Json::Array(inner) => out.extend(flatten(inner)),
            Json::Null => {}
            other => out.push(other.clone()),
        }
    }
    out
}

/// The elements among an element's children (flattened), and the text and
/// other values among them that aren't (whitespace between the tags is
/// neither): for a host whose element holds only elements, the second list
/// is what to warn about.
pub fn element_children(children: &[Json]) -> (Vec<Json>, Vec<String>) {
    let mut els = Vec::new();
    let mut stray = Vec::new();
    for c in flatten(children) {
        match &c {
            Json::Object(o) if o.get("type").and_then(Json::as_str) == Some("element") => els.push(c),
            Json::String(s) if s.trim().is_empty() => {}
            Json::String(s) => stray.push(s.trim().to_string()),
            _ => stray.push(c.to_string()),
        }
    }
    (els, stray)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_props_with_defaults() {
        let el = json!({"type": "element", "tag": "rect", "props": {"x": 3, "fill": "#ff0066", "gone": null}, "children": []});
        let p = Props::of(&el).unwrap();
        assert_eq!((p.num("x", 0.0), p.num("y", 7.0)), (3.0, 7.0));
        assert_eq!(p.color("fill"), Some([255, 0, 102, 255]));
        assert!(!p.has("gone"));
        let rec = json!({"w": 10});
        assert_eq!(Props::of(&rec).unwrap().num("w", 0.0), 10.0);
        assert!(Props::of(&json!(3)).is_none());
        assert_eq!(Props::default().str("a", "b"), "b");
    }

    #[test]
    fn colours() {
        assert_eq!(parse_hex(":ff0066"), Some([255, 0, 102, 255]));
        assert_eq!(parse_hex("f06"), Some([255, 0, 102, 255]));
        assert_eq!(parse_hex("#ff006680"), Some([255, 0, 102, 128]));
        assert_eq!(parse_hex("#ffé"), None);
        assert_eq!(color(&json!({"r": 1, "g": 2, "b": 3})), Some([1, 2, 3, 255]));
        assert_eq!(color(&json!({"r": 1, "g": 2, "b": 3, "a": 4})), Some([1, 2, 3, 4]));
    }

    #[test]
    fn children() {
        let kids = json!([" ", [{"type": "element", "tag": "a"}, null], "stray", 3]);
        let (els, stray) = element_children(kids.as_array().unwrap());
        assert_eq!(els, vec![json!({"type": "element", "tag": "a"})]);
        assert_eq!(stray, vec!["stray".to_string(), "3".to_string()]);
    }
}
