//! What a script's elements mean. Petal elements are data that a host draws,
//! so the same `<text>` is a Mixit text layer in a design and an unknown tag
//! in a petal-web-html page. A dialect names the host vocabulary: which tags
//! are HTML, which are components of a runtime file, and which attributes
//! change name on the way into React.

use petal::ast::{Stmt, StmtKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// A Mixit (petal-design) document: `import design`, `<vstack>`,
    /// `<instance of=…>`, `component(...)`, `design(doc, components, pages)`.
    Design,
    /// A petal-web-html page: HTML tags, `class`, `style="…"`, and clicks
    /// through `next_id()` / `clicked(id)` / `eid={id}`.
    Web,
    /// An immediate-mode `ui` or `bloom` script that draws with calls rather
    /// than elements. Converted as far as the language goes; the drawing has
    /// no React equivalent and is reported.
    Immediate,
    /// Anything else: HTML tags are host elements, other tags are components
    /// imported from the elements module.
    Generic,
}

impl Dialect {
    /// Guess from the imports and, failing that, from the elements used.
    pub fn detect(stmts: &[Stmt]) -> Dialect {
        let mut imports = Vec::new();
        for s in stmts {
            if let StmtKind::Import(d) = &s.kind {
                imports.push(d.module.trim_start_matches("./").to_string());
            }
        }
        let has = |m: &str| imports.iter().any(|i| i == m || i.starts_with(&format!("{m}/")));
        if has("design") || imports.iter().any(|i| i.starts_with("design_") || i.starts_with("design-")) {
            Dialect::Design
        } else if has("web") {
            Dialect::Web
        } else if has("ui") || has("bloom") {
            Dialect::Immediate
        } else {
            Dialect::Generic
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Dialect::Design => "design",
            Dialect::Web => "web",
            Dialect::Immediate => "immediate",
            Dialect::Generic => "generic",
        }
    }

    pub fn parse(s: &str) -> Option<Dialect> {
        Some(match s {
            "design" | "mixit" => Dialect::Design,
            "web" | "html" => Dialect::Web,
            "immediate" | "ui" => Dialect::Immediate,
            "generic" => Dialect::Generic,
            _ => return None,
        })
    }
}

/// What a tag becomes.
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    /// A DOM element, written lowercase (`div`, `button`).
    Host(String),
    /// A component exported by the dialect's runtime file.
    Runtime(String),
    /// `<instance of=…>`: a design component.
    Instance,
    /// `<slot>`: a design component's slot.
    Slot,
    /// A tag the dialect does not know: imported from the elements module.
    Unknown(String),
}

pub const HTML_TAGS: &[&str] = &[
    "a", "abbr", "article", "aside", "audio", "b", "blockquote", "body", "br", "button", "canvas",
    "caption", "code", "col", "dd", "details", "dialog", "div", "dl", "dt", "em", "fieldset",
    "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr",
    "i", "iframe", "img", "input", "label", "legend", "li", "main", "mark", "nav", "ol", "optgroup",
    "option", "p", "pre", "progress", "section", "select", "small", "source", "span", "strong",
    "sub", "summary", "sup", "svg", "path", "circle", "g", "line", "polyline", "polygon", "rect",
    "table", "tbody", "td", "textarea", "tfoot", "th", "thead", "time", "tr", "u", "ul", "video",
];

/// The elements of a Mixit document and the runtime component each is.
pub const DESIGN_TAGS: &[(&str, &str)] = &[
    ("page", "Page"),
    ("frame", "Frame"),
    ("vstack", "VStack"),
    ("hstack", "HStack"),
    ("zstack", "ZStack"),
    ("grid", "Grid"),
    ("spacer", "Spacer"),
    ("rect", "Rect"),
    ("ellipse", "Ellipse"),
    ("capsule", "Capsule"),
    ("line", "Line"),
    ("path", "Path"),
    ("text", "Text"),
    ("span", "Span"),
    ("image", "Image"),
    ("icon", "Icon"),
    ("canvas_doc", "CanvasDoc"),
    ("section", "Section"),
    ("specimen", "Specimen"),
    ("phone", "Phone"),
    ("phone_row", "PhoneRow"),
    ("note", "Note"),
    ("guideline", "Guideline"),
    ("do", "Do"),
    ("dont", "Dont"),
];

pub fn tag(dialect: Dialect, tag: &str) -> Tag {
    match dialect {
        Dialect::Design => {
            if tag == "instance" {
                return Tag::Instance;
            }
            if tag == "slot" {
                return Tag::Slot;
            }
            if tag == "br" {
                return Tag::Host("br".into());
            }
            match DESIGN_TAGS.iter().find(|(t, _)| *t == tag) {
                Some((_, c)) => Tag::Runtime(c.to_string()),
                None => Tag::Unknown(tag.to_string()),
            }
        }
        _ => {
            if HTML_TAGS.contains(&tag) {
                Tag::Host(tag.to_string())
            } else {
                Tag::Unknown(tag.to_string())
            }
        }
    }
}

/// The React name of an attribute on a host (DOM) element.
pub fn host_attr(dialect: Dialect, name: &str) -> String {
    match name {
        "class" => "className".into(),
        "for" => "htmlFor".into(),
        "eid" if dialect == Dialect::Web => "onClick".into(),
        "tabindex" => "tabIndex".into(),
        "readonly" => "readOnly".into(),
        "maxlength" => "maxLength".into(),
        "colspan" => "colSpan".into(),
        "rowspan" => "rowSpan".into(),
        "autofocus" => "autoFocus".into(),
        _ if name.starts_with("on_") || name.starts_with("on") && name.len() > 2 && !name.contains('_') && name[2..].chars().all(|c| c.is_ascii_lowercase()) => {
            // on_click / onclick → onClick
            let rest = name.trim_start_matches("on").trim_start_matches('_');
            format!("on{}", crate::names::pascal(rest))
        }
        _ => name.to_string(),
    }
}

/// The runtime file a dialect's components come from, and its contents.
pub fn runtime_file(dialect: Dialect) -> Option<(&'static str, &'static str)> {
    match dialect {
        Dialect::Design => Some(("petal-design.jsx", crate::DESIGN_RUNTIME)),
        _ => None,
    }
}

/// `style="display: flex; gap: 0.5rem"` → `{ display: "flex", gap: "0.5rem" }`.
pub fn css_object(css: &str) -> Option<String> {
    let mut parts = Vec::new();
    for decl in css.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let (k, v) = decl.split_once(':')?;
        let k = k.trim();
        let key = if k.starts_with("--") {
            crate::names::js_string(k)
        } else {
            let mut out = String::new();
            let mut up = false;
            for c in k.trim_start_matches('-').chars() {
                if c == '-' {
                    up = true;
                } else if up {
                    out.extend(c.to_uppercase());
                    up = false;
                } else {
                    out.push(c);
                }
            }
            if k.starts_with("-ms-") { out } else if k.starts_with('-') { crate::names::pascal(&out) } else { out }
        };
        parts.push(format!("{key}: {}", crate::names::js_string(v.trim())));
    }
    Some(format!("{{ {} }}", parts.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css() {
        assert_eq!(
            css_object("display: flex; gap: 0.5rem; margin-bottom: 1rem;").unwrap(),
            r#"{ display: "flex", gap: "0.5rem", marginBottom: "1rem" }"#
        );
        assert_eq!(host_attr(Dialect::Web, "on_click"), "onClick");
        assert_eq!(host_attr(Dialect::Web, "onclick"), "onClick");
        assert_eq!(host_attr(Dialect::Web, "class"), "className");
    }
}
