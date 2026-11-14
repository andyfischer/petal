//! Intents: edits stated against a document's *output*, and the resolution
//! ladder that turns one into a source edit.
//!
//! An [`Intent`] names an output element by its [`OutputKey`] and says what
//! should hold of it ("`#logo` has `x = 300`"). [`resolve`] finds where in
//! the program that can be made true, trying the most direct strategy first:
//!
//! 1. **Direct literal.** The field's value flows unchanged from one literal
//!    (`x={300}`, `let m = 96` … `x={m}`, a helper called with `x: 300`).
//!    Edit that literal.
//! 2. **Single site.** The field is not written, and the element literal that
//!    built the output built only it (fan-out 1). Add the attribute there,
//!    wherever that literal is.
//!
//! 3. **Shared site, chosen scope.** The literal or site built other outputs
//!    too. The scopes that verify come back as a
//!    [`Resolution::NeedsChoice`]: edit the shared literal (all of them
//!    change), edit the data entry a `map` or `for` read the value from, or
//!    write an override through the host's [`OverrideHook`]. A
//!    [`ScopePolicy`] can pick one instead ([`crate::scope`]).
//! 4. **Solve.** The field is computed: Petal's direct-manipulation solver
//!    finds which numbers of the document to change, holding the fields in
//!    [`ResolveOptions::hold`] ([`crate::solve`]). A value read per item
//!    from a data list is edited in the data first (step 3's data scope).
//! 5. **Concretize.** With the caller's consent
//!    ([`ResolveOptions::concretize`]), the call that made the output is
//!    replaced by the literal of its value, and the intent applied to that
//!    ([`crate::concretize`]).
//! 6. **Refuse with a reason** that names the place to edit by hand:
//!    [`Resolution::Unresolved`] (nothing applies), or
//!    [`Resolution::Refused`] (the edit lands in another file, or does not
//!    do what was asked when run).
//!
//! Every edit is verified by running the edited source: the intent must hold
//! in the new output, and no other output may change (an edit that changes
//! others too is reported as a choice, not made).

use petal::source_map::{FileId, SourceSpan};
use petal::static_value::StaticValue;

use std::rc::Rc;

use crate::output_map::{FieldProvenance, LiteralSource, OutputKey, OutputNode, PathSeg, path_string};
use crate::run::{DocumentRun, Runner};
use crate::scope::{OverrideHook, OverrideRequest, Scope, ScopePolicy, data_entry, use_site};
use crate::splice::{LiteralEdit, edit_span};

/// What the user wants to be true of the output.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// Output `target` has `field` (dotted for a field inside a record prop:
    /// `style.fill`) equal to `value`.
    SetField { target: OutputKey, field: String, value: StaticValue },
    /// Output `target` has no `field`.
    RemoveField { target: OutputKey, field: String },
}

impl Intent {
    pub fn target(&self) -> &OutputKey {
        match self {
            Intent::SetField { target, .. } | Intent::RemoveField { target, .. } => target,
        }
    }

    pub fn field(&self) -> &str {
        match self {
            Intent::SetField { field, .. } | Intent::RemoveField { field, .. } => field,
        }
    }
}

/// Which rung of the ladder an edit came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Step 1: the literal the field flows from.
    DirectLiteral,
    /// Step 2: the element literal that built the output, in place.
    SingleSite,
    /// Step 3: an edit in a chosen scope (the shared literal, the data
    /// entry, an override).
    Scope(Scope),
    /// Step 4: numbers the direct-manipulation solver chose.
    Solve,
    /// Step 5: the output written out as source, then edited.
    Concretize,
}

/// A verified source edit.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEdit {
    /// The whole new source.
    pub source: String,
    pub step: Step,
    /// The literal that was edited, in the old source.
    pub span: SourceSpan,
    /// What the edit does, for the user ("set `96` on line 1 to `120`").
    pub description: String,
    /// The other outputs (or fields of this one) the edit changes too, as
    /// the rerun saw them: empty for an edit that changes only what was
    /// asked. A solve or an edit of a shared literal may change others.
    pub also_changes: Vec<String>,
}

/// One way to go on when an intent needs the user's say.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub label: String,
    /// The scope this choice edits in.
    pub scope: Scope,
    /// The edit that makes the intent hold this way, verified to make it
    /// hold (but it changes other outputs too). `None` for a choice the host
    /// carries out some other way.
    pub edit: Option<ResolvedEdit>,
}

/// The outcome of [`resolve`].
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// Apply this edit (it is verified).
    Edit(ResolvedEdit),
    /// The intent already holds; nothing to do.
    AlreadyHolds,
    /// The intent can be satisfied, but only by changing other outputs too,
    /// or in a way the user should pick.
    NeedsChoice { reason: String, options: Vec<Choice> },
    /// No strategy of steps 1 and 2 applies (the value is computed, the
    /// element has no recorded origin).
    Unresolved { reason: String, at: Option<SourceSpan> },
    /// The edit would land outside what may be written, or did not do what
    /// was asked when run.
    Refused { reason: String },
}

/// Limits on where [`resolve`] may write, and how it chooses.
#[derive(Clone)]
pub struct ResolveOptions {
    /// Allow edits outside the document's own file (the entry file). Off by
    /// default: such an edit is refused with a reason naming the file. The
    /// returned source is always the entry file's, so a host that turns this
    /// on must write other files itself; today this only changes the
    /// refusal into [`Resolution::Unresolved`].
    pub allow_other_files: bool,
    /// How a shared site's scope is chosen (step 3): ask (the default), or
    /// take the first verified scope of a list.
    pub scope_policy: ScopePolicy,
    /// The host's way of writing a per-instance override, if it has one.
    pub overrides: Option<Rc<dyn OverrideHook>>,
    /// Solve computed numeric fields with direct manipulation (step 4). On
    /// by default; a solve re-runs the document many times.
    pub solve: bool,
    /// Fields of the target to keep where they are while solving (a drag
    /// along `x` holds `y`).
    pub hold: Vec<String>,
    /// The caller's consent to write the output out as source when nothing
    /// else applies (step 5). Off by default.
    pub concretize: bool,
}

impl Default for ResolveOptions {
    fn default() -> Self {
        ResolveOptions {
            allow_other_files: false,
            scope_policy: ScopePolicy::Ask,
            overrides: None,
            solve: true,
            hold: Vec::new(),
            concretize: false,
        }
    }
}

impl std::fmt::Debug for ResolveOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolveOptions")
            .field("allow_other_files", &self.allow_other_files)
            .field("scope_policy", &self.scope_policy)
            .field("overrides", &self.overrides.is_some())
            .field("solve", &self.solve)
            .field("hold", &self.hold)
            .field("concretize", &self.concretize)
            .finish()
    }
}

/// Resolve `intent` against `run`, the latest run of `source` with `runner`.
pub fn resolve(runner: &Runner, run: &DocumentRun, intent: &Intent, opts: &ResolveOptions) -> Resolution {
    let res = resolve_in_place(runner, run, intent, opts);
    if !opts.concretize || !matches!(res, Resolution::Unresolved { .. }) {
        return res;
    }
    match concretize_and_apply(runner, run, intent, opts) {
        Ok(r) => r,
        Err(why) => match res {
            Resolution::Unresolved { reason, at } => Resolution::Unresolved { reason: format!("{reason}; {why}"), at },
            other => other,
        },
    }
}

/// Step 5 on its own: write the output `intent` names out as source (the
/// caller's consent is calling this), then resolve the intent against the
/// written-out document. The edit returned does both.
pub fn concretize_and_apply(
    runner: &Runner,
    run: &DocumentRun,
    intent: &Intent,
    opts: &ResolveOptions,
) -> Result<Resolution, String> {
    let node = run.map.find(intent.target()).ok_or_else(|| format!("the document has no output {}", intent.target()))?;
    let written = crate::concretize::concretize(runner, run, node)?;
    let after = runner.run(&written.source)?;
    let Some(new_node) = after.map.at_path(&node.path) else {
        return Err("the written-out document lost the output".into());
    };
    let retarget = match intent {
        Intent::SetField { field, value, .. } => {
            Intent::SetField { target: new_node.key.clone(), field: field.clone(), value: value.clone() }
        }
        Intent::RemoveField { field, .. } => Intent::RemoveField { target: new_node.key.clone(), field: field.clone() },
    };
    let inner = ResolveOptions { concretize: false, ..opts.clone() };
    let wrap = |e: ResolvedEdit| ResolvedEdit {
        source: e.source,
        step: Step::Concretize,
        span: written.span,
        description: format!("{}, then {}", written.description, e.description),
        also_changes: e.also_changes,
    };
    Ok(match resolve_in_place(runner, &after, &retarget, &inner) {
        Resolution::Edit(e) => Resolution::Edit(wrap(e)),
        Resolution::AlreadyHolds => Resolution::Edit(ResolvedEdit {
            source: written.source,
            step: Step::Concretize,
            span: written.span,
            description: written.description,
            also_changes: Vec::new(),
        }),
        Resolution::NeedsChoice { reason, options } => Resolution::NeedsChoice {
            reason,
            options: options.into_iter().map(|c| Choice { edit: c.edit.map(wrap), ..c }).collect(),
        },
        other => other,
    })
}

/// Steps 1 to 4 and 6.
fn resolve_in_place(runner: &Runner, run: &DocumentRun, intent: &Intent, opts: &ResolveOptions) -> Resolution {
    let Some(node) = run.map.find(intent.target()) else {
        return Resolution::Unresolved {
            reason: format!("the document has no output {}", intent.target()),
            at: None,
        };
    };
    let (head, rest) = split_field(intent.field());
    if let Some(r) = already_holds(run, node, intent) {
        return r;
    }
    let tag = &node.tag;
    let Some(site) = &node.site else {
        return Resolution::Unresolved {
            reason: format!("<{tag}> at {} was not built by an element literal, so it has no source to edit", path_string(&node.path)),
            at: None,
        };
    };
    let field = node.field(head);
    match (intent, field.map(|f| &f.provenance)) {
        // Step 1: the literal the value flows from.
        (Intent::SetField { value, .. }, Some(FieldProvenance::Literal(lit))) => {
            if lit.constant.is_some() && !rest.is_empty() {
                return Resolution::Refused {
                    reason: format!(
                        "`{head}` is {} (line {}), which has no field `{}`",
                        lit.constant.as_ref().map(StaticValue::to_source).unwrap_or_default(),
                        lit.span.start.line,
                        rest.trim_start_matches('.')
                    ),
                };
            }
            direct_literal(runner, run, node, intent, lit, LiteralEdit::Set { path: rest, value }, opts)
        }
        (Intent::RemoveField { .. }, Some(FieldProvenance::Literal(lit))) if !rest.is_empty() => {
            if lit.constant.is_some() {
                return Resolution::AlreadyHolds;
            }
            direct_literal(runner, run, node, intent, lit, LiteralEdit::Remove { path: rest }, opts)
        }
        (Intent::SetField { value, .. }, Some(FieldProvenance::Computed { at, reason })) => {
            computed(runner, run, node, intent, value, reason, *at, opts)
        }
        (Intent::RemoveField { .. }, Some(FieldProvenance::Computed { at, reason })) if !rest.is_empty() => {
            Resolution::Unresolved {
                reason: format!("{reason}; edit it by hand"),
                at: *at,
            }
        }
        // Step 2: on the element literal itself. Setting a field it does not
        // write, or removing an attribute.
        (Intent::SetField { value, .. }, None) => {
            let value = nest(rest, value.clone());
            let path = format!(".{head}");
            single_site(runner, run, node, intent, LiteralEdit::Set { path: &path, value: &value }, opts)
        }
        (Intent::RemoveField { .. }, _) => {
            let path = format!(".{head}");
            let _ = site;
            single_site(runner, run, node, intent, LiteralEdit::Remove { path: &path }, opts)
        }
    }
}

/// A computed field: the data entry it was read from (step 3's data scope,
/// which changes only this output), then a solve (step 4), else a reason.
#[allow(clippy::too_many_arguments)]
fn computed(
    runner: &Runner,
    run: &DocumentRun,
    node: &OutputNode,
    intent: &Intent,
    value: &StaticValue,
    reason: &str,
    at: Option<SourceSpan>,
    opts: &ResolveOptions,
) -> Resolution {
    let mut why = vec![reason.to_string()];
    let (head, rest) = split_field(intent.field());
    if let Some(r) = edit_data(runner, run, node, intent, head, rest, value, opts) {
        match r {
            Ok(res) => return res,
            Err(e) => why.push(e),
        }
    }
    if opts.solve {
        match crate::solve::number(value) {
            None => why.push(format!("{} is not a number, so it cannot be solved for", value.to_source())),
            Some(target) => {
                match crate::solve::solve_field(runner, run, node, intent.field(), target, &opts.hold, opts.allow_other_files) {
                    Ok(s) => {
                        let edit = ResolvedEdit {
                            source: s.source,
                            step: Step::Solve,
                            span: s.span,
                            description: s.description,
                            also_changes: Vec::new(),
                        };
                        let others = match verify(runner, run, node, intent, &edit.source) {
                            Verified::Holds { others } => others,
                            Verified::Fails(f) => {
                                return Resolution::Refused {
                                    reason: format!("the solver's edit ({}) fails: {f}", edit.description),
                                };
                            }
                        };
                        let edit = ResolvedEdit { also_changes: others, ..edit };
                        if edit.also_changes.is_empty() {
                            return Resolution::Edit(edit);
                        }
                        // It changes other outputs too: when the host can
                        // write an override for this one alone, that is a
                        // choice.
                        let more = other_scopes(runner, run, node, intent, opts);
                        if more.is_empty() {
                            return Resolution::Edit(edit);
                        }
                        let reason = format!("solving moves {} too", edit.also_changes.join(", "));
                        let mut options =
                            vec![Choice { label: "Change them all".into(), scope: Scope::EditShared, edit: Some(edit) }];
                        options.extend(more);
                        return choose(reason, options, &opts.scope_policy);
                    }
                    Err(e) => why.push(e),
                }
            }
        }
    }
    let place = at.map(|s| format!(" on line {}", s.start.line)).unwrap_or_default();
    Resolution::Unresolved { reason: format!("{}; edit it by hand{place}", why.join("; ")), at }
}

/// Step 3's data scope: the entry of the data list the field was read from.
/// `None` when the field is not read from one; `Some(Err)` says why the
/// entry could not be edited.
#[allow(clippy::too_many_arguments)]
fn edit_data(
    runner: &Runner,
    run: &DocumentRun,
    node: &OutputNode,
    intent: &Intent,
    head: &str,
    rest: &str,
    value: &StaticValue,
    opts: &ResolveOptions,
) -> Option<Result<Resolution, String>> {
    let entry = data_entry(run, node, head).ok()?;
    let edit = match data_edit(runner, run, node, intent, &entry, rest, value, opts) {
        Ok(e) => e,
        Err(e) => return Some(Err(e)),
    };
    Some(Ok(match edit {
        DataEdit::Refused(r) => r,
        DataEdit::Edit(e) if e.also_changes.is_empty() => Resolution::Edit(e),
        DataEdit::Edit(e) => Resolution::NeedsChoice {
            reason: format!("changing {} also changes {}", e.description, e.also_changes.join(", ")),
            options: vec![Choice { label: "Change the data entry".into(), scope: Scope::EditData, edit: Some(e) }],
        },
    }))
}

enum DataEdit {
    Edit(ResolvedEdit),
    Refused(Resolution),
}

#[allow(clippy::too_many_arguments)]
fn data_edit(
    runner: &Runner,
    run: &DocumentRun,
    node: &OutputNode,
    intent: &Intent,
    entry: &crate::scope::DataEntry,
    rest: &str,
    value: &StaticValue,
    opts: &ResolveOptions,
) -> Result<DataEdit, String> {
    let line = entry.span.start.line;
    if let Some(r) = file_check(run, entry.span.file, &format!("the data `{}`", entry.path.trim_start_matches('.')), opts) {
        return Ok(DataEdit::Refused(r));
    }
    let path = format!("{}{rest}", entry.path);
    let source = edit_span(&run.source, &entry.span, LiteralEdit::Set { path: &path, value })
        .map_err(|e| format!("could not edit the data entry `{path}` on line {line}: {e}"))?;
    let description = format!("set the data entry `{path}` on line {line} to `{}`", value.to_source());
    match verify(runner, run, node, intent, &source) {
        Verified::Fails(f) => Err(format!("editing the data entry `{path}` on line {line} does not work: {f}")),
        Verified::Holds { others } => Ok(DataEdit::Edit(ResolvedEdit {
            source,
            step: Step::Scope(Scope::EditData),
            span: entry.span,
            description,
            also_changes: others,
        })),
    }
}

/// `style.fill` → (`style`, `.fill`).
fn split_field(field: &str) -> (&str, &str) {
    match field.find('.') {
        Some(i) => (&field[..i], &field[i..]),
        None => (field, ""),
    }
}

/// `value` under the record path `rest` (`.a.b`): `{a: {b: value}}`.
fn nest(rest: &str, value: StaticValue) -> StaticValue {
    rest.split('.')
        .filter(|s| !s.is_empty())
        .rev()
        .fold(value, |v, k| StaticValue::Record(vec![(k.to_string(), v)]))
}

fn already_holds(run: &DocumentRun, node: &OutputNode, intent: &Intent) -> Option<Resolution> {
    let el = run.node_value(node)?;
    let current = field_of(&el, intent.field());
    let holds = match intent {
        Intent::SetField { value, .. } => current.is_some_and(|c| same(c, value)),
        Intent::RemoveField { .. } => current.is_none(),
    };
    holds.then_some(Resolution::AlreadyHolds)
}

fn file_check(run: &DocumentRun, file: FileId, what: &str, opts: &ResolveOptions) -> Option<Resolution> {
    if file == FileId(0) {
        return None;
    }
    let name = petal::provenance::file_source(run.program(), file)
        .map(|(_, n)| n.to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("file {}", file.0));
    let reason = format!("{what} is written in {name}, not in this document; edit it there by hand");
    Some(if opts.allow_other_files { Resolution::Unresolved { reason, at: None } } else { Resolution::Refused { reason } })
}

fn literal_text(run: &DocumentRun, span: &SourceSpan) -> String {
    let t = petal::provenance::span_text(run.program(), span).unwrap_or_default();
    let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > 40 { format!("{}…", t.chars().take(40).collect::<String>()) } else { t }
}

fn direct_literal(
    runner: &Runner,
    run: &DocumentRun,
    node: &OutputNode,
    intent: &Intent,
    lit: &LiteralSource,
    edit: LiteralEdit<'_>,
    opts: &ResolveOptions,
) -> Resolution {
    let text = literal_text(run, &lit.span);
    let field = intent.field();
    if let Some(r) = file_check(run, lit.file(), &format!("`{field}`'s value `{text}`"), opts) {
        return r;
    }
    let source = match edit_span(&run.source, &lit.span, edit) {
        Ok(s) => s,
        Err(e) => return Resolution::Refused { reason: format!("could not edit `{text}` on line {}: {e}", lit.span.start.line) },
    };
    let description = match edit {
        LiteralEdit::Set { path, value } => {
            format!("set `{text}`{path} on line {} to `{}`", lit.span.start.line, value.to_source())
        }
        LiteralEdit::Remove { path } => format!("remove `{}` from `{text}` on line {}", path.trim_start_matches('.'), lit.span.start.line),
    };
    let edit = ResolvedEdit { source, step: Step::DirectLiteral, span: lit.span, description, also_changes: Vec::new() };
    finish(runner, run, node, intent, edit, lit.shared, &format!("`{text}` on line {}", lit.span.start.line), opts)
}

fn single_site(
    runner: &Runner,
    run: &DocumentRun,
    node: &OutputNode,
    intent: &Intent,
    edit: LiteralEdit<'_>,
    opts: &ResolveOptions,
) -> Resolution {
    let site = node.site.as_ref().expect("checked by resolve");
    let tag = &node.tag;
    let Some(span) = site.span else {
        return Resolution::Unresolved { reason: format!("<{tag}>'s element literal has no source position"), at: None };
    };
    if let Some(r) = file_check(run, span.file, &format!("<{tag}>"), opts) {
        return r;
    }
    let source = match edit_span(&run.source, &span, edit) {
        Ok(s) => s,
        Err(e) => return Resolution::Refused { reason: format!("could not edit <{tag}> on line {}: {e}", span.start.line) },
    };
    let description = match edit {
        LiteralEdit::Set { path, value } => {
            format!("set `{}` on <{tag}> (line {}) to `{}`", path.trim_start_matches('.'), span.start.line, value.to_source())
        }
        LiteralEdit::Remove { path } => {
            format!("remove `{}` from <{tag}> (line {})", path.trim_start_matches('.'), span.start.line)
        }
    };
    let edit = ResolvedEdit { source, step: Step::SingleSite, span, description, also_changes: Vec::new() };
    finish(runner, run, node, intent, edit, site.fan_out, &format!("<{tag}> on line {}", span.start.line), opts)
}

/// Verify `edit`, and turn it into a resolution: the edit, a choice when it
/// (statically or by rerun) changes other outputs, or a refusal.
#[allow(clippy::too_many_arguments)]
fn finish(
    runner: &Runner,
    run: &DocumentRun,
    node: &OutputNode,
    intent: &Intent,
    edit: ResolvedEdit,
    shared: usize,
    what: &str,
    opts: &ResolveOptions,
) -> Resolution {
    match verify(runner, run, node, intent, &edit.source) {
        Verified::Fails(reason) => Resolution::Refused { reason },
        Verified::Holds { others } if shared > 1 || !others.is_empty() => {
            let reason = if shared > 1 {
                format!("{what} makes {shared} outputs, so changing it changes all of them")
            } else {
                format!("changing {what} also changes {}", others.join(", "))
            };
            let label = if shared > 1 { format!("Change all {shared}") } else { "Change them all".to_string() };
            let shared_edit = ResolvedEdit { step: Step::Scope(Scope::EditShared), also_changes: others, ..edit };
            let mut options = vec![Choice { label, scope: Scope::EditShared, edit: Some(shared_edit) }];
            options.extend(other_scopes(runner, run, node, intent, opts));
            choose(reason, options, &opts.scope_policy)
        }
        Verified::Holds { .. } => Resolution::Edit(edit),
    }
}

/// The scopes besides editing the shared site that make `intent` hold:
/// the data entry, and the host's override.
fn other_scopes(runner: &Runner, run: &DocumentRun, node: &OutputNode, intent: &Intent, opts: &ResolveOptions) -> Vec<Choice> {
    let mut out = Vec::new();
    let (head, rest) = split_field(intent.field());
    if let Intent::SetField { value, .. } = intent
        && let Ok(entry) = data_entry(run, node, head)
        && let Ok(DataEdit::Edit(e)) = data_edit(runner, run, node, intent, &entry, rest, value, opts)
    {
        out.push(Choice { label: "Change only this one's data".into(), scope: Scope::EditData, edit: Some(e) });
    }
    if let Some(hook) = &opts.overrides {
        let req = OverrideRequest { run, node, intent, use_site: use_site(run, node) };
        if let Some(Ok(o)) = hook.write_override(&req)
            && let Verified::Holds { others } = verify(runner, run, node, intent, &o.source)
        {
            let edit = ResolvedEdit {
                source: o.source,
                step: Step::Scope(Scope::Override),
                span: o.span,
                description: o.description,
                also_changes: others,
            };
            out.push(Choice { label: "Override this one".into(), scope: Scope::Override, edit: Some(edit) });
        }
    }
    out
}

/// Apply `policy` to the verified `options`.
fn choose(reason: String, options: Vec<Choice>, policy: &ScopePolicy) -> Resolution {
    if let ScopePolicy::Prefer(order) = policy {
        for scope in order {
            let pick = options.iter().find(|c| {
                c.scope == *scope
                    && c.edit.as_ref().is_some_and(|e| *scope == Scope::EditShared || e.also_changes.is_empty())
            });
            if let Some(Choice { edit: Some(e), .. }) = pick {
                return Resolution::Edit(e.clone());
            }
        }
    }
    Resolution::NeedsChoice { reason, options }
}

/// The outcome of re-running an edited source.
#[derive(Debug, Clone, PartialEq)]
pub enum Verified {
    /// The intent holds in the new output. `others` lists the other outputs
    /// that changed too (empty when none did).
    Holds { others: Vec<String> },
    /// It does not hold, or the edited source does not run; why.
    Fails(String),
}

/// Run `new_source` and check that `intent` holds of the output `node` of
/// `before` names, and which other outputs changed.
pub fn verify(runner: &Runner, before: &DocumentRun, node: &OutputNode, intent: &Intent, new_source: &str) -> Verified {
    let after = match runner.run(new_source) {
        Ok(a) => a,
        Err(e) => return Verified::Fails(format!("the edited document does not run: {e}")),
    };
    // Setting `id` renames the output.
    let key = match intent {
        Intent::SetField { field, value, .. } if field == "id" => match value {
            StaticValue::Str(s) => OutputKey::Id(s.clone()),
            StaticValue::Int(n) => OutputKey::Id(n.to_string()),
            _ => OutputKey::Path(node.path.clone()),
        },
        Intent::RemoveField { field, .. } if field == "id" => OutputKey::Path(node.path.clone()),
        _ => node.key.clone(),
    };
    let Some(new_node) = after.map.find(&key) else {
        return Verified::Fails(format!("the edited document no longer has the output {key}"));
    };
    let Some(el) = after.node_value(new_node) else {
        return Verified::Fails("the edited output has no literal form".into());
    };
    let got = field_of(&el, intent.field());
    let ok = match intent {
        Intent::SetField { value, .. } => got.is_some_and(|g| same(g, value)),
        Intent::RemoveField { .. } => got.is_none(),
    };
    if !ok {
        let got = got.map(StaticValue::to_source).unwrap_or_else(|| "no value".into());
        let want = match intent {
            Intent::SetField { value, .. } => value.to_source(),
            Intent::RemoveField { .. } => "no value".into(),
        };
        return Verified::Fails(format!(
            "after the edit <{}>'s `{}` is {got}, not {want}; nothing was changed",
            node.tag,
            intent.field()
        ));
    }
    Verified::Holds { others: changed_others(before, &after, node, intent) }
}

/// The outputs other than `node` whose value differs between the runs,
/// described for the user, or a general note when the result changed in a
/// way no element accounts for.
fn changed_others(before: &DocumentRun, after: &DocumentRun, node: &OutputNode, intent: &Intent) -> Vec<String> {
    let mut out = Vec::new();
    // The target's other fields.
    if let (Some(mut want), Some(m)) = (shallow(before, node), after.map.find(&node.key)) {
        apply_intent(&mut want, intent);
        if let (StaticValue::Element { props: p1, .. }, Some(StaticValue::Element { props: p2, .. })) =
            (&want, shallow(after, m))
        {
            for (k, v) in p1 {
                let other = p2.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v);
                if !other.is_some_and(|o| same(o, v)) {
                    out.push(format!("`{k}` of the same <{}>", node.tag));
                }
            }
        }
    }
    for n in &before.map.nodes {
        if n.key == node.key || n.path == node.path {
            continue;
        }
        let Some(m) = after.map.find(&n.key) else {
            out.push(format!("<{}> {}", n.tag, n.key));
            continue;
        };
        if let (Some(a), Some(b)) = (shallow(before, n), shallow(after, m))
            && !same(&a, &b)
        {
            out.push(format!("<{}> {}", n.tag, n.key));
        }
    }
    if out.is_empty() {
        // Anything outside the elements (a project record, text).
        let (Some(mut want), Some(got)) = (
            before.static_outputs().into_iter().collect::<Option<Vec<_>>>(),
            after.static_outputs().into_iter().collect::<Option<Vec<_>>>(),
        ) else {
            return out;
        };
        if let Some(el) = node_at_mut(&mut want, &node.path) {
            apply_intent(el, intent);
        }
        if want.len() != got.len() || want.iter().zip(&got).any(|(a, b)| !same(a, b)) {
            out.push("other values in the result".into());
        }
    }
    out
}

/// An element node's own value: its tag, props and text, without its child
/// elements (they are nodes of their own).
fn shallow(run: &DocumentRun, n: &OutputNode) -> Option<StaticValue> {
    match run.node_value(n)? {
        StaticValue::Element { tag, props, children } => Some(StaticValue::Element {
            tag,
            props: props.into_iter().filter(|(_, v)| !holds_element(v)).collect(),
            children: children.into_iter().filter(|c| !holds_element(c)).collect(),
        }),
        _ => None,
    }
}

fn holds_element(v: &StaticValue) -> bool {
    match v {
        StaticValue::Element { .. } => true,
        StaticValue::List(xs) => xs.iter().any(holds_element),
        StaticValue::Record(fs) => fs.iter().any(|(_, v)| holds_element(v)),
        _ => false,
    }
}

fn node_at_mut<'a>(outputs: &'a mut [StaticValue], path: &[PathSeg]) -> Option<&'a mut StaticValue> {
    let (PathSeg::Index(first), rest) = path.split_first()? else { return None };
    let mut cur = outputs.get_mut(*first)?;
    for seg in rest {
        cur = match (seg, cur) {
            (PathSeg::Index(i), StaticValue::List(xs)) => xs.get_mut(*i)?,
            (PathSeg::Field(k), StaticValue::Record(fs)) => fs.iter_mut().rev().find(|(n, _)| n == k).map(|(_, v)| v)?,
            (PathSeg::Prop(k), StaticValue::Element { props, .. }) => {
                props.iter_mut().rev().find(|(n, _)| n == k).map(|(_, v)| v)?
            }
            (PathSeg::Child(i), StaticValue::Element { children, .. }) => children.get_mut(*i)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Make `intent` hold of the element value `el`.
fn apply_intent(el: &mut StaticValue, intent: &Intent) {
    let StaticValue::Element { props, .. } = el else { return };
    let parts: Vec<&str> = intent.field().split('.').collect();
    match intent {
        Intent::SetField { value, .. } => set_in(props, &parts, value.clone()),
        Intent::RemoveField { .. } => remove_in(props, &parts),
    }
}

fn set_in(fields: &mut Vec<(String, StaticValue)>, parts: &[&str], value: StaticValue) {
    let Some((k, rest)) = parts.split_first() else { return };
    let slot = fields.iter_mut().rev().find(|(n, _)| n == k);
    match (slot, rest.is_empty()) {
        (Some((_, v)), true) => *v = value,
        (Some((_, StaticValue::Record(inner))), false) => set_in(inner, rest, value),
        (Some((_, v)), false) => {
            let mut inner = Vec::new();
            set_in(&mut inner, rest, value);
            *v = StaticValue::Record(inner);
        }
        (None, true) => fields.push((k.to_string(), value)),
        (None, false) => {
            let mut inner = Vec::new();
            set_in(&mut inner, rest, value);
            fields.push((k.to_string(), StaticValue::Record(inner)));
        }
    }
}

fn remove_in(fields: &mut Vec<(String, StaticValue)>, parts: &[&str]) {
    let Some((k, rest)) = parts.split_first() else { return };
    if rest.is_empty() {
        fields.retain(|(n, _)| n != k);
    } else if let Some((_, StaticValue::Record(inner))) = fields.iter_mut().rev().find(|(n, _)| n == k) {
        remove_in(inner, rest);
    }
}

/// Field `field` (dotted) of element value `el`.
pub fn field_of<'a>(el: &'a StaticValue, field: &str) -> Option<&'a StaticValue> {
    let StaticValue::Element { props, .. } = el else { return None };
    let mut parts = field.split('.');
    let first = parts.next()?;
    let mut cur = props.iter().rev().find(|(k, _)| k == first).map(|(_, v)| v)?;
    for p in parts {
        let StaticValue::Record(fs) = cur else { return None };
        cur = fs.iter().rev().find(|(k, _)| k == p).map(|(_, v)| v)?;
    }
    Some(cur)
}

/// Whether two values are the same as far as an output is concerned: an
/// integer and a float of equal value are, a color and the `{r, g, b}`
/// record it is at run time are, and record fields may be in any order.
pub fn same(a: &StaticValue, b: &StaticValue) -> bool {
    use StaticValue as S;
    match (a, b) {
        (S::Int(x), S::Float(y)) | (S::Float(y), S::Int(x)) => (*x as f64) == *y,
        (S::Color { .. }, _) => same(&color_record(a), b),
        (_, S::Color { .. }) => same(a, &color_record(b)),
        (S::List(xs), S::List(ys)) => xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| same(x, y)),
        (S::Record(xs), S::Record(ys)) => records_same(xs, ys),
        (S::Element { tag: t1, props: p1, children: c1 }, S::Element { tag: t2, props: p2, children: c2 }) => {
            t1 == t2 && records_same(p1, p2) && c1.len() == c2.len() && c1.iter().zip(c2).all(|(x, y)| same(x, y))
        }
        _ => a == b,
    }
}

fn records_same(xs: &[(String, StaticValue)], ys: &[(String, StaticValue)]) -> bool {
    let last = |fs: &[(String, StaticValue)], k: &str| fs.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let keys = |fs: &[(String, StaticValue)]| {
        let mut ks: Vec<String> = fs.iter().map(|(k, _)| k.clone()).collect();
        ks.sort();
        ks.dedup();
        ks
    };
    let (kx, ky) = (keys(xs), keys(ys));
    kx == ky && kx.iter().all(|k| matches!((last(xs, k), last(ys, k)), (Some(a), Some(b)) if same(&a, &b)))
}

fn color_record(c: &StaticValue) -> StaticValue {
    match c {
        StaticValue::Color { r, g, b, a } => {
            let mut fs = vec![
                ("r".to_string(), StaticValue::Int(*r as i64)),
                ("g".to_string(), StaticValue::Int(*g as i64)),
                ("b".to_string(), StaticValue::Int(*b as i64)),
            ];
            if let Some(a) = a {
                fs.push(("a".to_string(), StaticValue::Int(*a as i64)));
            }
            StaticValue::Record(fs)
        }
        other => other.clone(),
    }
}
