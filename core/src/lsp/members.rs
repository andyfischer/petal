//! Enum members, as the editor asks about them: what may follow `Kind.`,
//! what a variant value answers (`k.shape`, `op.place`, `k.name`), and where
//! each of those is declared.
//!
//! Everything here is read off the declarations and the text around the
//! cursor; nothing is compiled or run. A path such as `Kind.park`,
//! `mod.Kind.park` or `k` (a variable annotated `: Kind`, or initialized from
//! a variant) resolves to a [`Member`], and completion, hover and
//! go-to-definition are each a reading of that.

use crate::ast::{EnumVariant, Expr, ExprKind};
use crate::source_map::SourceSpan;

use crate::lsp::document::{Analysis, Definition, DefinitionKind};
use crate::lsp::lsp_types::{CompletionItem, CompletionItemKind};

use crate::program::VARIANT_NAME_FIELD as NAME_FIELD;

// ---------------------------------------------------------------------------
// Shapes: what a declaration says
// ---------------------------------------------------------------------------

/// An enum's variants, in declaration order.
#[derive(Debug, Clone, Default)]
pub struct EnumShape {
    pub variants: Vec<VariantShape>,
}

#[derive(Debug, Clone)]
pub struct VariantShape {
    pub name: String,
    /// Payload fields, in constructor order.
    pub fields: Vec<String>,
    /// Constant data: each key with its expression as the source spells it.
    pub data: Vec<(String, String)>,
    pub span: SourceSpan,
}

impl EnumShape {
    pub fn of(variants: &[EnumVariant], src: &[char]) -> Self {
        let text = |e: &Expr| -> String {
            let (start, end) = (e.span.start.offset as usize, e.span.end.offset as usize);
            match src.get(start..end) {
                Some(chars) if !chars.is_empty() && !chars.contains(&'\n') => chars.iter().collect(),
                _ => "…".to_string(),
            }
        };
        EnumShape {
            variants: variants
                .iter()
                .map(|v| VariantShape {
                    name: v.name.clone(),
                    fields: v.fields.clone(),
                    data: v.data.iter().map(|d| (d.key.clone(), text(&d.value))).collect(),
                    span: v.span,
                })
                .collect(),
        }
    }

    pub fn variant(&self, name: &str) -> Option<&VariantShape> {
        self.variants.iter().find(|v| v.name == name)
    }

    /// The payload-free variants: what `Enum.all` lists and `Enum.from` finds.
    fn plain(&self) -> impl Iterator<Item = &VariantShape> {
        self.variants.iter().filter(|v| v.fields.is_empty())
    }
}

impl VariantShape {
    /// `red`, or `custom(r, g, b)` for a variant that is constructed.
    pub fn signature(&self) -> String {
        if self.fields.is_empty() {
            self.name.clone()
        } else {
            format!("{}({})", self.name, self.fields.join(", "))
        }
    }

    fn datum(&self, key: &str) -> Option<&str> {
        let (_, text) = self.data.iter().find(|(k, _)| k == key)?;
        Some(text)
    }
}

// ---------------------------------------------------------------------------
// Paths: what the source spells
// ---------------------------------------------------------------------------

/// One name of a dotted path, and whether it was called: `fight` in
/// `Op.fight("x", true)` is, `park` in `Kind.park` is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub name: String,
    pub called: bool,
}

/// What a declaration says of a variable's or parameter's value.
#[derive(Debug, Clone)]
pub enum ValueHint {
    /// `k: Kind` — a value of the type the path names.
    Annotation(Vec<Segment>),
    /// `let k = Kind.park` — whatever the path is.
    Init(Vec<Segment>),
    /// `for k in Kind.all` — an element of what the path is.
    ElementOf(Vec<Segment>),
}

impl ValueHint {
    pub fn annotation(type_name: &str) -> Self {
        ValueHint::Annotation(
            type_name
                .split('.')
                .map(|name| Segment {
                    name: name.to_string(),
                    called: false,
                })
                .collect(),
        )
    }
}

/// The dotted path an expression spells, when it is nothing but one:
/// `Kind.park`, `m.Kind.park`, `Op.fight("x", true)`, `Kind.from(s)`.
pub fn expr_path(expr: &Expr) -> Option<Vec<Segment>> {
    match &expr.kind {
        ExprKind::Ident(name) => Some(vec![Segment {
            name: name.clone(),
            called: false,
        }]),
        ExprKind::FieldAccess { object, field } => {
            let mut path = expr_path(object)?;
            path.push(Segment {
                name: field.clone(),
                called: false,
            });
            Some(path)
        }
        ExprKind::Call { function, .. } => {
            let mut path = expr_path(function)?;
            let last = path.last_mut()?;
            if last.called {
                return None; // `f()()`: the result of a result
            }
            last.called = true;
            Some(path)
        }
        _ => None,
    }
}

/// The dotted path written before the name at the position: `[Kind]` for a
/// cursor on `park` in `Kind.park` or right after the dot of `Kind.`, and
/// `[m, Kind]` in `m.Kind.park`. Empty when the name stands alone, `None`
/// when what is before the dot is not a path (`xs[0].`, `"a".`).
pub fn path_before_position(source: &str, line: u32, character: u32) -> Option<Vec<Segment>> {
    let chars: Vec<char> = source.lines().nth(line as usize)?.chars().collect();
    let mut at = (character as usize).min(chars.len());
    while at > 0 && is_ident_char(chars[at - 1]) {
        at -= 1;
    }
    let mut path: Vec<Segment> = Vec::new();
    while at > 0 && chars[at - 1] == '.' {
        let mut end = at - 1;
        // A call's arguments: step back over the balanced parentheses.
        let called = end > 0 && chars[end - 1] == ')';
        if called {
            let mut depth = 0usize;
            loop {
                end -= 1;
                match chars[end] {
                    ')' => depth += 1,
                    '(' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
                if end == 0 {
                    return None;
                }
            }
        }
        let mut start = end;
        while start > 0 && is_ident_char(chars[start - 1]) {
            start -= 1;
        }
        if start == end || chars[start].is_ascii_digit() {
            return None;
        }
        path.insert(
            0,
            Segment {
                name: chars[start..end].iter().collect(),
                called,
            },
        );
        at = start;
    }
    Some(path)
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

// ---------------------------------------------------------------------------
// Resolution: what a path is
// ---------------------------------------------------------------------------

/// An enum declaration, and the file it is in (`None`: this document).
#[derive(Clone, Copy)]
pub struct EnumAt<'a> {
    pub def: &'a Definition,
    pub shape: &'a EnumShape,
    pub uri: Option<&'a str>,
}

/// What a path is, as far as the declarations say.
#[derive(Clone, Copy)]
pub enum Member<'a> {
    /// The enum itself: `Kind`, `m.Kind`, or a variable initialized to one.
    Enum(EnumAt<'a>),
    /// One variant's value: `Kind.park`, `Op.fight("x", true)`.
    Variant(EnumAt<'a>, &'a VariantShape),
    /// A payload variant's constructor, not yet called: `Op.fight`.
    Constructor(EnumAt<'a>, &'a VariantShape),
    /// Some variant of the enum, not known which: `k` of `k: Kind`,
    /// `Kind.from(s)`, an element of `Kind.all`.
    Value(EnumAt<'a>),
    /// `Kind.all`: the list of the payload-free variants.
    All(EnumAt<'a>),
    /// `Kind.from`, the lookup function.
    From(EnumAt<'a>),
}

/// A cursor position in the document, 0-based as the protocol gives it.
#[derive(Clone, Copy)]
pub struct At {
    pub line: u32,
    pub character: u32,
}

impl Analysis {
    /// What `path` is at `at`, when it leads to an enum.
    pub fn member(&self, path: &[Segment], at: At) -> Option<Member<'_>> {
        self.member_within(path, at, 0)
    }

    fn member_within(&self, path: &[Segment], at: At, depth: usize) -> Option<Member<'_>> {
        let (first, rest) = path.split_first()?;
        if first.called || depth > 4 {
            return None;
        }
        // A variable or parameter: what its declaration says it holds.
        if let Some(def) = self.definition_in_scope(&first.name, at)
            && def.kind != DefinitionKind::Enum
        {
            let base = match def.hint.as_ref()? {
                ValueHint::Annotation(of) => match self.member_within(of, at, depth + 1)? {
                    Member::Enum(e) => Member::Value(e),
                    _ => return None,
                },
                ValueHint::Init(of) => self.member_within(of, at, depth + 1)?,
                ValueHint::ElementOf(of) => match self.member_within(of, at, depth + 1)? {
                    Member::All(e) => Member::Value(e),
                    _ => return None,
                },
            };
            return member_of(base, rest);
        }
        let (found, rest) = self.enum_named(path)?;
        member_of(Member::Enum(found), rest)
    }

    /// The enum the front of `path` names — declared here, imported bare, or
    /// reached through a module qualifier — and the rest of the path.
    fn enum_named<'p>(&self, path: &'p [Segment]) -> Option<(EnumAt<'_>, &'p [Segment])> {
        let (first, rest) = path.split_first()?;
        let local = self
            .definitions
            .iter()
            .find(|d| d.name == first.name && d.kind == DefinitionKind::Enum);
        if let Some(def) = local {
            let shape = def.shape.as_ref()?;
            return Some((EnumAt { def, shape, uri: None }, rest));
        }
        let through_module = self.qualifier(&first.name).and_then(|module| {
            let (second, rest) = rest.split_first()?;
            Some((module.export(&second.name)?, rest))
        });
        let (found, rest) = match through_module {
            Some(found) => found,
            None => (self.imported(&first.name)?, rest),
        };
        let shape = found.def.shape.as_ref()?;
        let at = EnumAt {
            def: &found.def,
            shape,
            uri: Some(found.uri.as_deref().unwrap_or("")),
        };
        Some((at, rest))
    }

    /// The declaration a use of `name` at `at` most plausibly reaches: the
    /// innermost parameter or loop variable whose function or loop the
    /// position is in, else the last declaration above the position, else
    /// the first one anywhere (a hoisted `fn` or `enum`).
    pub fn definition_in_scope(&self, name: &str, at: At) -> Option<&Definition> {
        let (line, column) = (at.line + 1, at.character + 1);
        let pos = (line, column);
        let named = || self.definitions.iter().filter(|d| d.name == name);
        let scoped = |d: &&Definition| {
            matches!(d.kind, DefinitionKind::Parameter | DefinitionKind::Loop)
                && (d.span.start.line, d.span.start.column) <= pos
                && pos <= (d.span.end.line, d.span.end.column)
        };
        let lines = |d: &&Definition| d.span.end.line - d.span.start.line;
        named()
            .filter(scoped)
            .min_by_key(lines)
            .or_else(|| {
                named()
                    .filter(|d| !matches!(d.kind, DefinitionKind::Parameter | DefinitionKind::Loop))
                    .filter(|d| (d.span.start.line, d.span.start.column) <= pos)
                    .last()
            })
            .or_else(|| named().next())
    }
}

/// `base` followed by `rest`: one more name at most means something here.
fn member_of<'a>(base: Member<'a>, rest: &[Segment]) -> Option<Member<'a>> {
    let Some((next, after)) = rest.split_first() else {
        return Some(base);
    };
    if !after.is_empty() {
        return None; // a field of a field: a value like any other
    }
    let Member::Enum(e) = base else {
        return None;
    };
    Some(match e.shape.variant(&next.name) {
        Some(v) if v.fields.is_empty() || next.called => Member::Variant(e, v),
        Some(v) => Member::Constructor(e, v),
        None if next.name == "all" && !next.called => Member::All(e),
        None if next.name == "from" && next.called => Member::Value(e),
        None if next.name == "from" => Member::From(e),
        None => return None,
    })
}

// ---------------------------------------------------------------------------
// Readings: completion, hover, definition
// ---------------------------------------------------------------------------

/// Where a member is declared: the span, in the file `uri` (`None`: this
/// document; `Some("")`: a file that could not be located).
pub struct Declared<'a> {
    pub uri: Option<&'a str>,
    pub span: SourceSpan,
}

impl<'a> Member<'a> {
    fn of(&self) -> EnumAt<'a> {
        match *self {
            Member::Enum(e)
            | Member::Variant(e, _)
            | Member::Constructor(e, _)
            | Member::Value(e)
            | Member::All(e)
            | Member::From(e) => e,
        }
    }

    /// What may be written after this and a `.`.
    pub fn completions(&self) -> Vec<CompletionItem> {
        let e = self.of();
        let enum_name = &e.def.name;
        let item = |label: &str, kind: CompletionItemKind, detail: String| CompletionItem {
            label: label.to_string(),
            kind: Some(kind),
            detail: Some(detail),
        };
        let field = |label: &str, detail: String| item(label, CompletionItemKind::Field, detail);
        match *self {
            Member::Enum(_) => {
                let mut items: Vec<CompletionItem> = e
                    .shape
                    .variants
                    .iter()
                    .map(|v| {
                        item(
                            &v.name,
                            CompletionItemKind::EnumMember,
                            format!("{enum_name}.{}", v.signature()),
                        )
                    })
                    .collect();
                items.push(field(
                    "all",
                    format!("{enum_name}.all — its payload-free variants, in order"),
                ));
                items.push(item(
                    "from",
                    CompletionItemKind::Function,
                    format!("{enum_name}.from(name) — the variant with that name, or nil"),
                ));
                items
            }
            Member::Variant(_, v) => {
                let of = format!("{enum_name}.{}", v.name);
                let mut items: Vec<CompletionItem> = Vec::new();
                for f in &v.fields {
                    items.push(field(f, format!("payload field of {of}")));
                }
                for (key, text) in &v.data {
                    items.push(field(key, format!("{of}.{key} = {text}")));
                }
                items.push(field(NAME_FIELD, format!("{of}.name = \"{}\"", v.name)));
                items
            }
            Member::Value(_) => {
                let mut items: Vec<CompletionItem> = Vec::new();
                let mut seen: Vec<&str> = Vec::new();
                // Data keys first: every variant has them.
                for v in &e.shape.variants {
                    for (key, _) in &v.data {
                        if !seen.contains(&key.as_str()) {
                            seen.push(key);
                            items.push(field(key, format!("constant data of {enum_name}")));
                        }
                    }
                }
                items.push(field(NAME_FIELD, "the variant's name".to_string()));
                for v in &e.shape.variants {
                    for f in &v.fields {
                        if !seen.contains(&f.as_str()) {
                            seen.push(f);
                            let owners: Vec<String> = e
                                .shape
                                .variants
                                .iter()
                                .filter(|o| o.fields.contains(f))
                                .map(|o| format!("{enum_name}.{}", o.name))
                                .collect();
                            items.push(field(f, format!("payload field of {}", owners.join(", "))));
                        }
                    }
                }
                items
            }
            // A function and a list: nothing of the enum's follows the dot.
            Member::Constructor(..) | Member::All(_) | Member::From(_) => Vec::new(),
        }
    }

    /// Hover text for `name` written after this and a `.`.
    pub fn hover(&self, name: &str) -> Option<String> {
        let e = self.of();
        let enum_name = &e.def.name;
        match *self {
            Member::Enum(_) => match e.shape.variant(name) {
                Some(v) => {
                    let mut text = format!("(variant) {enum_name}.{}", v.signature());
                    for (key, value) in &v.data {
                        text.push_str(&format!("\n  {key}: {value}"));
                    }
                    Some(text)
                }
                None if name == "all" => {
                    let plain: Vec<String> = e
                        .shape
                        .plain()
                        .map(|v| format!("{enum_name}.{}", v.name))
                        .collect();
                    Some(format!(
                        "(enum member) {enum_name}.all\n// the payload-free variants, in declaration order\n[{}]",
                        plain.join(", ")
                    ))
                }
                None if name == "from" => Some(format!(
                    "(enum member) {enum_name}.from(name)\n// the payload-free variant with that name, or nil"
                )),
                None => None,
            },
            Member::Variant(_, v) => {
                let of = format!("{enum_name}.{}", v.name);
                if v.fields.iter().any(|f| f == name) {
                    Some(format!("(payload field) {name}\n// of {enum_name}.{}", v.signature()))
                } else if let Some(text) = v.datum(name) {
                    Some(format!("(constant data) {of}.{name} = {text}"))
                } else if name == NAME_FIELD {
                    Some(format!("(field) {of}.name = \"{}\"", v.name))
                } else {
                    None
                }
            }
            Member::Value(_) => {
                let with_datum: Vec<String> = e
                    .shape
                    .variants
                    .iter()
                    .filter_map(|v| Some(format!("  {}: {}", v.name, v.datum(name)?)))
                    .collect();
                let with_field: Vec<String> = e
                    .shape
                    .variants
                    .iter()
                    .filter(|v| v.fields.iter().any(|f| f == name))
                    .map(|v| format!("{enum_name}.{}", v.signature()))
                    .collect();
                if !with_datum.is_empty() {
                    Some(format!(
                        "(constant data) {enum_name}.{name}\n{}",
                        with_datum.join("\n")
                    ))
                } else if !with_field.is_empty() {
                    Some(format!("(payload field) {name}\n// of {}", with_field.join(", ")))
                } else if name == NAME_FIELD {
                    Some(format!("(field) name\n// the name of the {enum_name} variant, as a string"))
                } else {
                    None
                }
            }
            Member::Constructor(..) | Member::All(_) | Member::From(_) => None,
        }
    }

    /// Where `name`, written after this and a `.`, is declared.
    pub fn declaration(&self, name: &str) -> Option<Declared<'a>> {
        let e = self.of();
        let declares = |v: &&VariantShape| {
            v.fields.iter().any(|f| f == name) || v.datum(name).is_some() || name == NAME_FIELD
        };
        let span = match *self {
            Member::Enum(_) => match e.shape.variant(name) {
                Some(v) => v.span,
                None if name == "all" || name == "from" => e.def.span,
                None => return None,
            },
            Member::Variant(_, v) => Some(v).filter(declares)?.span,
            // The first variant that declares it; `name` is the enum's own.
            Member::Value(_) if name == NAME_FIELD => e.def.span,
            Member::Value(_) => e.shape.variants.iter().find(declares)?.span,
            Member::Constructor(..) | Member::All(_) | Member::From(_) => return None,
        };
        Some(Declared { uri: e.uri, span })
    }
}
