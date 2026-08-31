//! The font registry: which faces this machine can draw, and how wide they are.
//!
//! Garden started with two faces compiled into the binary — a monospace one
//! for everything code-shaped, a proportional one for panel prose — and a
//! `FontRole` enum was enough to name them. A panel script can now ask for any
//! family installed on the machine (`font("Helvetica Neue")`), which is a
//! different shape of problem: the set of names is not known at compile time,
//! and a [`crate::TextStyle`] is `Copy` (it rides along in every text
//! primitive), so it cannot carry a `String`.
//!
//! So faces are *interned*: [`resolve`] turns a name into a small [`FontId`]
//! that a style can carry by value, and the shaper and the measurer both look
//! the family back up through [`family_of`]. Ids are stable for the life of
//! the process, and the two built-ins keep fixed ids so a default-constructed
//! style still means the monospace face.
//!
//! Everything here is process-global and lazily built, because it is all a
//! pure function of the machine's installed fonts: enumerating families and
//! measuring a face's advances is expensive enough that no caller should do it
//! twice, and none of it can change while Garden runs.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use glyphon::fontdb;

/// An interned font family, as carried by [`crate::TextStyle`].
///
/// Values are opaque and only meaningful to this module. [`FontId::MONO`] and
/// [`FontId::UI`] are fixed so the embedded faces can be named without a
/// lookup; every other id comes from [`resolve`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontId(u32);

impl FontId {
    /// The embedded monospace face (JetBrains Mono) — what every run Garden
    /// itself draws uses, and the default of [`crate::TextStyle`].
    pub const MONO: FontId = FontId(0);
    /// The embedded proportional face (Inter) — a panel script's `font: "ui"`.
    pub const UI: FontId = FontId(1);

    /// Is this one of the two faces compiled into the binary?
    pub fn is_embedded(self) -> bool {
        self == FontId::MONO || self == FontId::UI
    }
}

/// The name a script wrote to get [`FontId::MONO`].
pub const MONO_NAME: &str = "mono";
/// The name a script wrote to get [`FontId::UI`].
pub const UI_NAME: &str = "ui";

/// Interned family names, indexed by [`FontId`]. Entries 0 and 1 are the
/// embedded faces' real family names (read back from the font data, not
/// hardcoded); the rest are canonical system family names as fontdb spells
/// them.
struct Registry {
    names: Vec<String>,
    /// Lowercased request → id, so `"helvetica neue"` and `"Helvetica Neue"`
    /// intern to the same face rather than two ids drawing the same glyphs.
    by_name: HashMap<String, FontId>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let db = crate::text::full_db();
        let mut names = vec![
            db.mono.clone().unwrap_or_else(|| "monospace".to_string()),
            db.ui
                .clone()
                .or_else(|| db.mono.clone())
                .unwrap_or_else(|| "sans-serif".to_string()),
        ];
        let mut by_name = HashMap::new();
        // Both the role name a script writes and the face's real family name
        // resolve to the built-in id, so `font("JetBrains Mono")` and
        // `font("mono")` are the same face rather than two.
        by_name.insert(MONO_NAME.to_string(), FontId::MONO);
        by_name.insert("monospace".to_string(), FontId::MONO);
        by_name.insert(UI_NAME.to_string(), FontId::UI);
        for (i, name) in names.iter().enumerate() {
            by_name
                .entry(name.to_lowercase())
                .or_insert(FontId(i as u32));
        }
        names.shrink_to_fit();
        Mutex::new(Registry { names, by_name })
    })
}

/// Resolve a font spec to the face that will actually be drawn.
///
/// `spec` is a single name or a CSS-style fallback list (`"Helvetica, ui"`):
/// the first name this machine can draw wins. `mono`/`monospace` and `ui`
/// select the embedded faces; anything else is matched case-insensitively
/// against the installed families. A spec that resolves to nothing degrades to
/// [`FontId::MONO`] — the same direction the measurement side degrades in, so
/// a script that names a face this machine lacks measures and draws the same
/// thing rather than disagreeing with itself.
pub fn resolve(spec: &str) -> FontId {
    match try_resolve(spec) {
        Some(id) => id,
        None => {
            warn_unresolved(spec);
            FontId::MONO
        }
    }
}

/// Specs this process was asked for and could not draw, in first-seen order.
///
/// The degradation to [`FontId::MONO`] is deliberate — a panel that names a
/// face this machine lacks should still draw, and still measure the way it
/// draws — but it is also *silent*, and silence is how `font("serif")` spends
/// an afternoon looking like a layout bug: the text is there, it is legible,
/// it is simply not the typeface anyone asked for. So the fallback is recorded
/// as well as warned about, and the debug server reports the list.
pub fn unresolved_specs() -> Vec<String> {
    unresolved()
        .lock()
        .expect("unresolved font specs poisoned")
        .clone()
}

fn unresolved() -> &'static Mutex<Vec<String>> {
    static UNRESOLVED: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    UNRESOLVED.get_or_init(|| Mutex::new(Vec::new()))
}

/// Warn on stderr the first time `spec` fails to resolve, and remember it.
///
/// Once per *name*, not once per draw: a panel asks for its font every frame,
/// so warning per call would bury the log under sixty lines a second and the
/// warning would be worse than the silence it replaced.
fn warn_unresolved(spec: &str) {
    let mut seen = unresolved().lock().expect("unresolved font specs poisoned");
    if seen.iter().any(|s| s == spec) {
        return;
    }
    seen.push(spec.to_string());
    eprintln!(
        "garden-render: no font matches {spec:?} on this machine — drawing it \
         in the default monospace face instead. Name an installed family, or \
         one of the built-in roles ({MONO_NAME:?}, {UI_NAME:?})."
    );
}

/// [`resolve`] without the fallback: `None` when this machine can draw no name
/// in `spec`.
///
/// The distinction matters to a caller that has to *report* whether the face
/// exists — a script asking `font("Papyrus")` on a machine without it should
/// get its own name back and be measured with the default face, not be told it
/// is now holding the monospace one.
pub fn try_resolve(spec: &str) -> Option<FontId> {
    for name in spec.split(',') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if let Some(id) = resolve_one(name) {
            return Some(id);
        }
    }
    None
}

/// [`resolve`] for a single family name: `None` when this machine has no such
/// family, so [`resolve`] can try the next entry in a fallback list.
fn resolve_one(name: &str) -> Option<FontId> {
    let key = name.to_lowercase();
    let mut reg = registry().lock().expect("font registry poisoned");
    if let Some(id) = reg.by_name.get(&key) {
        return Some(*id);
    }
    // Not interned yet: ask the database. `canonical_family` is what makes the
    // lookup case- and whitespace-insensitive; the id is interned under the
    // canonical spelling so the shaper gets a name fontdb will match.
    let canonical = crate::text::canonical_family(&key)?;
    let id = FontId(reg.names.len() as u32);
    reg.names.push(canonical.clone());
    reg.by_name.insert(key, id);
    reg.by_name.insert(canonical.to_lowercase(), id);
    Some(id)
}

/// The family name `id` shapes with. `None` only for an id this process never
/// handed out.
pub fn family_of(id: FontId) -> Option<String> {
    let reg = registry().lock().expect("font registry poisoned");
    reg.names.get(id.0 as usize).cloned()
}

/// Every family this machine can draw, sorted, deduplicated, with the two
/// role names a script can also write (`mono`, `ui`) first — the list behind a
/// script's `fonts()`, for a font picker or a diagnostic.
///
/// Families whose name starts with a dot are left out. macOS ships dozens
/// (`.Aqua Kana`, `.Apple Color Emoji UI`, the `.CJK Symbols Fallback *` set):
/// they are internal fallback faces, not typefaces anyone picks, and listing
/// them buries the real ones. They are still *resolvable* by name — hiding a
/// face from the picker is not the same as refusing to draw it.
pub fn available_families() -> Vec<String> {
    let mut names = vec![MONO_NAME.to_string(), UI_NAME.to_string()];
    names.extend(
        crate::text::full_db()
            .families
            .iter()
            .filter(|name| !name.starts_with('.'))
            .cloned(),
    );
    names
}

/// Per-codepoint advance ratios (glyph advance ÷ font size) for ASCII in this
/// face at this weight and slant — the table a script's `text_width` sums.
///
/// Weight and slant are part of the key because they are part of the answer:
/// a real bold cut is wider than its regular, and measuring the regular would
/// put every centered or right-aligned bold run in the wrong place.
///
/// Measuring shapes 95 glyphs through cosmic-text, so results are memoized;
/// the answer is a pure function of the installed font files.
pub fn advance_ratios(id: FontId, weight: u16, italic: bool) -> Vec<f64> {
    static CACHE: OnceLock<Mutex<HashMap<(FontId, u16, bool), Vec<f64>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache
        .lock()
        .expect("font metrics cache poisoned")
        .get(&(id, weight, italic))
    {
        return hit.clone();
    }
    let ratios = crate::text::measure_family_advances(id, weight, italic);
    cache
        .lock()
        .expect("font metrics cache poisoned")
        .insert((id, weight, italic), ratios.clone());
    ratios
}

/// Weight at or above which a run is bold — CSS semibold. Below this, nothing
/// is emboldened, real or synthetic.
pub const BOLD_THRESHOLD: u16 = 600;

/// The cut a run is actually shaped with: `(weight, italic)` of the real face
/// the shaper will pick, which is not always the one asked for.
///
/// Two reasons it differs. The embedded roles are **pinned** to the cuts Garden
/// ships, and that pinning is load-bearing: the database also holds every
/// family installed on the machine, and a developer's machine very often has
/// JetBrains Mono or Inter installed under the *same family name* as the
/// embedded copy. Asking for a weight Garden does not embed would then quietly
/// shape the editor in a stranger's font file, with different advances than the
/// ones every column position was computed from. (Insertion order settles the
/// regular weight — the embedded faces load first and win fontdb's tie-break —
/// so only the weights Garden lacks need pinning.)
///
/// For a named family it is fontdb's CSS matching, reported back as the chosen
/// face's *own* declared attributes. cosmic-text will only shape with a face
/// whose declared weight equals the requested one, and plenty of real fonts
/// declare something unexpected — macOS's Apple Chancery declares weight 0 —
/// so asking for 400 and taking whatever comes back is how such a family
/// silently renders in a fallback face instead of itself.
pub fn shaping_cut(id: FontId, weight: u16, italic: bool) -> (u16, bool) {
    const REGULAR: u16 = crate::REGULAR_WEIGHT;
    const BOLD: u16 = 700;
    match id {
        // JetBrains Mono Regular is the only monospace cut compiled in.
        FontId::MONO => (REGULAR, italic),
        // Inter ships Regular and Bold; anything else snaps to the nearer one.
        FontId::UI => match weight >= BOLD_THRESHOLD {
            true => (BOLD, italic),
            false => (REGULAR, italic),
        },
        _ => named_shaping_cut(id, weight, italic).unwrap_or((weight, italic)),
    }
}

/// [`shaping_cut`] for a named family, memoized — it queries the whole font
/// database, and a draw loop asks per run per frame.
fn named_shaping_cut(id: FontId, weight: u16, italic: bool) -> Option<(u16, bool)> {
    static CACHE: OnceLock<Mutex<HashMap<(FontId, u16, bool), Option<(u16, bool)>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (id, weight, italic);
    if let Some(hit) = cache.lock().expect("font cut cache poisoned").get(&key) {
        return *hit;
    }
    let cut = family_of(id).and_then(|family| crate::text::db_best_cut(&family, weight, italic));
    cache
        .lock()
        .expect("font cut cache poisoned")
        .insert(key, cut);
    cut
}

/// Must this run's bold be faked by over-drawing?
///
/// Yes when it is bold but the face the shaper will actually use is not —
/// the embedded JetBrains Mono, which ships Regular only, or a system family
/// with a single weight. A family that *does* have a real bold is left alone:
/// smearing a genuine bold cut only blurs it.
pub fn needs_synthetic_bold(id: FontId, weight: u16, italic: bool) -> bool {
    weight >= BOLD_THRESHOLD && shaping_cut(id, weight, italic).0 < BOLD_THRESHOLD
}

/// A fontdb weight/style pair for a CSS weight and slant.
pub(crate) fn db_query_attrs(weight: u16, italic: bool) -> (fontdb::Weight, fontdb::Style) {
    (
        fontdb::Weight(weight),
        match italic {
            true => fontdb::Style::Italic,
            false => fontdb::Style::Normal,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_role_names_resolve_to_the_embedded_faces() {
        assert_eq!(resolve("mono"), FontId::MONO);
        assert_eq!(resolve("monospace"), FontId::MONO);
        assert_eq!(resolve("ui"), FontId::UI);
        // Case and surrounding space are not part of a font's identity.
        assert_eq!(resolve("  UI  "), FontId::UI);
    }

    #[test]
    fn an_unknown_family_degrades_to_mono() {
        assert_eq!(resolve("Definitely Not A Font 12345"), FontId::MONO);
        // …but only after the rest of a fallback list has been tried.
        assert_eq!(resolve("Definitely Not A Font 12345, ui"), FontId::UI);
    }

    /// The degradation is reported, not silent. A spec that resolves to
    /// nothing lands in the list exactly once however many times it is asked
    /// for — a panel asks for its font every frame.
    #[test]
    fn an_unresolvable_spec_is_recorded_once() {
        let spec = "Nothing Named This 98765";
        for _ in 0..3 {
            assert_eq!(resolve(spec), FontId::MONO);
        }
        let reported = unresolved_specs();
        assert_eq!(
            reported.iter().filter(|s| *s == spec).count(),
            1,
            "expected exactly one entry for {spec:?} in {reported:?}"
        );
        // A face that *does* resolve stays out of it.
        resolve(UI_NAME);
        assert!(!unresolved_specs().iter().any(|s| s == UI_NAME));
    }

    /// The point of the whole module: a family that is not compiled in still
    /// resolves to a face of its own, and keeps a stable id.
    #[test]
    fn a_system_family_interns_to_a_stable_id_of_its_own() {
        let families = crate::text::full_db().families.clone();
        // Pick a family that isn't one of the embedded ones, so the assertion
        // is about system font discovery rather than the built-ins.
        let Some(name) = families
            .iter()
            .find(|n| resolve(n).is_embedded() == false)
            .cloned()
        else {
            // A machine with no installed fonts beyond the embedded pair (some
            // CI containers). Nothing to assert; the fallback path is covered
            // by `an_unknown_family_degrades_to_mono`.
            return;
        };
        let id = resolve(&name);
        assert!(!id.is_embedded(), "{name} should intern as its own face");
        assert_eq!(id, resolve(&name), "ids must be stable across lookups");
        assert_eq!(id, resolve(&name.to_uppercase()), "lookup is case-folded");
        assert_eq!(family_of(id).as_deref(), Some(name.as_str()));
    }

    /// The embedded roles must not be reachable by the machine's own copy of a
    /// same-named family. If a developer has JetBrains Mono installed, asking
    /// for a weight Garden doesn't embed would otherwise shape the editor in
    /// that copy — with advances the whole column arithmetic was not built on.
    #[test]
    fn the_embedded_roles_are_pinned_to_the_cuts_garden_ships() {
        assert_eq!(shaping_cut(FontId::MONO, 400, false), (400, false));
        assert_eq!(shaping_cut(FontId::MONO, 700, false), (400, false));
        assert_eq!(shaping_cut(FontId::MONO, 900, false), (400, false));
        // Inter ships both cuts, so bold is real and anything between snaps.
        assert_eq!(shaping_cut(FontId::UI, 400, false), (400, false));
        assert_eq!(shaping_cut(FontId::UI, 500, false), (400, false));
        assert_eq!(shaping_cut(FontId::UI, 900, false), (700, false));
    }

    #[test]
    fn only_a_face_without_a_bold_cut_is_smeared() {
        // JetBrains Mono Regular is the only monospace cut compiled in.
        assert!(needs_synthetic_bold(FontId::MONO, 700, false));
        // Inter Bold is compiled in, so smearing it would only blur it.
        assert!(!needs_synthetic_bold(FontId::UI, 700, false));
        // Nothing below semibold is emboldened either way.
        assert!(!needs_synthetic_bold(FontId::MONO, 500, false));
        assert!(!needs_synthetic_bold(FontId::UI, 500, false));
    }

    /// Bold Inter is wider than regular Inter, so the two cuts must measure
    /// differently — otherwise a bold UI label centered from `text_width`
    /// lands off-center while nothing about the drawing looks wrong.
    #[test]
    fn a_real_bold_cut_measures_wider_than_its_regular() {
        let regular = crate::ui_ascii_advance_ratios();
        let bold = crate::ui_bold_ascii_advance_ratios();
        assert_ne!(
            regular, bold,
            "Inter Bold must not measure as Inter Regular"
        );
        let sum = |t: &[f64]| t[0x20..0x7f].iter().sum::<f64>();
        assert!(
            sum(&bold) > sum(&regular),
            "bold ASCII should be wider overall (regular {}, bold {})",
            sum(&regular),
            sum(&bold)
        );
    }

    #[test]
    fn advance_ratios_are_measured_and_memoized() {
        let a = advance_ratios(FontId::UI, 400, false);
        let b = advance_ratios(FontId::UI, 400, false);
        assert_eq!(a, b);
        assert_eq!(a.len(), 128);
        // Control codes draw nothing; printable glyphs do.
        assert_eq!(a[0x09], 0.0);
        assert!(a['W' as usize] > a['i' as usize], "Inter is proportional");
    }
}
