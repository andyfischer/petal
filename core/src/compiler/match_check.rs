//! Compile-time checks on a `match`, made on its arms' resolved patterns.
//!
//! - **Or-patterns bind alike.** Every alternative of `a | b` binds the same
//!   names, so an arm's body can use them whichever alternative matched.
//!   Otherwise a compile error.
//! - **Exhaustiveness.** A `match` whose arms are all variants of one enum,
//!   with no arm that takes everything else, must cover every variant of that
//!   enum. Otherwise a compile error naming the missing ones. A guarded arm
//!   covers nothing, and a payload pattern covers its variant only when none
//!   of its sub-patterns can fail.
//! - **Unreachable arms.** An arm that follows one matching every value, or
//!   that repeats a literal or variant an earlier arm already took, never
//!   runs. A warning: the program is still well defined.
//!
//! The checks are syntactic. They know nothing of the subject's type, so a
//! `match` on strings is never held to be exhaustive or not.

use super::*;
use crate::ir_display::format_pattern;
use std::collections::{BTreeSet, HashSet};

/// The values one alternative is keyed on: two alternatives with the same key
/// are tried against the same values.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Nil,
    Bool(bool),
    /// A number that is a whole `i64`, written as an int or as a float: `2`
    /// and `2.0` match the same values.
    Int(i64),
    Float(u64),
    Str(String),
    /// A variant, by its tag (`Kind.park`).
    Variant(String),
}

/// Floats at or beyond this magnitude are not compared as whole numbers.
const EXACT_FLOAT_LIMIT: f64 = 9_007_199_254_740_992.0; // 2^53

fn key_of(pattern: &Pattern) -> Option<Key> {
    match pattern {
        Pattern::Literal(Literal::Nil) => Some(Key::Nil),
        Pattern::Literal(Literal::Bool(b)) => Some(Key::Bool(*b)),
        Pattern::Literal(Literal::Int(n)) => Some(Key::Int(*n)),
        // NaN equals nothing, itself included, so it is keyed on nothing.
        Pattern::Literal(Literal::Float(f)) if f.is_nan() => None,
        Pattern::Literal(Literal::Float(f)) => {
            Some(if f.fract() == 0.0 && f.abs() < EXACT_FLOAT_LIMIT {
                Key::Int(*f as i64)
            } else {
                Key::Float(f.to_bits())
            })
        }
        Pattern::Literal(Literal::String(s)) => Some(Key::Str(s.clone())),
        Pattern::Variant { name, .. } => Some(Key::Variant(name.clone())),
        _ => None,
    }
}

/// Whether the alternative matches every value of its key: a literal always,
/// a variant pattern when none of its sub-patterns can fail.
fn covers_its_key(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Literal(_) => true,
        Pattern::Variant { fields, .. } => fields.iter().all(Pattern::is_irrefutable),
        _ => false,
    }
}

fn quoted_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| format!("`{i}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

impl Compiler {
    /// Run every check on one `match`. `span` is the whole expression;
    /// `metas` holds each arm's pattern with its variants resolved to tags.
    pub(super) fn check_match(&mut self, span: SourceSpan, arms: &[MatchArm], metas: &[MatchArmMeta]) {
        for (arm, meta) in arms.iter().zip(metas) {
            self.check_or_bindings(&meta.pattern, arm.pattern_span);
        }
        let catch_all = self.check_reachability(arms, metas);
        if !catch_all {
            self.check_exhaustive(span, arms, metas);
        }
    }

    /// Every alternative of an or-pattern binds the same names.
    fn check_or_bindings(&mut self, pattern: &Pattern, span: SourceSpan) {
        match pattern {
            Pattern::Or(alts) => {
                let names = |p: &Pattern| p.bound_names().into_iter().collect::<BTreeSet<_>>();
                let first = alts.first().map(names).unwrap_or_default();
                for alt in alts.iter().skip(1) {
                    let these = names(alt);
                    if these == first {
                        continue;
                    }
                    let odd: Vec<String> = first.symmetric_difference(&these).cloned().collect();
                    let binder = if first.contains(&odd[0]) { &alts[0] } else { alt };
                    let other = if first.contains(&odd[0]) { alt } else { &alts[0] };
                    self.error_at(
                        span,
                        format!(
                            "every alternative of an or-pattern must bind the same names, \
                             but `{}` binds {} and `{}` does not",
                            format_pattern(binder),
                            quoted_list(&odd[..1]),
                            format_pattern(other),
                        ),
                    );
                    break;
                }
                for alt in alts {
                    self.check_or_bindings(alt, span);
                }
            }
            Pattern::Variant { fields, .. } => {
                for f in fields {
                    self.check_or_bindings(f, span);
                }
            }
            Pattern::List { elements, .. } => {
                for e in elements {
                    self.check_or_bindings(e, span);
                }
            }
            Pattern::Record(fields) => {
                for (_, p) in fields {
                    self.check_or_bindings(p, span);
                }
            }
            Pattern::Wildcard | Pattern::Literal(_) | Pattern::Variable(_) => {}
        }
    }

    /// Warn on each arm, and each alternative, that can never be the one to
    /// match. Returns whether some arm takes every value.
    fn check_reachability(&mut self, arms: &[MatchArm], metas: &[MatchArmMeta]) -> bool {
        // Keys an earlier unguarded arm matches in full.
        let mut taken: HashSet<Key> = HashSet::new();
        let mut catch_all = false;
        for (arm, meta) in arms.iter().zip(metas) {
            if catch_all {
                self.warn_at(
                    arm.pattern_span,
                    "this arm can never run: an earlier arm of the match already matches \
                     every value"
                        .to_string(),
                );
                continue;
            }
            let alts = meta.pattern.alternatives();
            let mut here: HashSet<Key> = HashSet::new();
            let mut dead: Vec<String> = Vec::new();
            for alt in &alts {
                let Some(key) = key_of(alt) else { continue };
                if taken.contains(&key) || here.contains(&key) {
                    dead.push(format_pattern(alt));
                } else if covers_its_key(alt) {
                    here.insert(key);
                }
            }
            if !dead.is_empty() && dead.len() == alts.len() {
                self.warn_at(
                    arm.pattern_span,
                    format!(
                        "this arm can never run: an earlier arm already matches {}",
                        quoted_list(&dead)
                    ),
                );
            } else if !dead.is_empty() {
                self.warn_at(
                    arm.pattern_span,
                    format!(
                        "{} can never match here: an earlier pattern already matches it",
                        quoted_list(&dead)
                    ),
                );
            }
            if arm.guard.is_none() {
                taken.extend(here);
                catch_all = meta.pattern.is_irrefutable();
            }
        }
        catch_all
    }

    /// A match over the variants of one enum, with no arm for "anything
    /// else", must have an arm for each variant.
    fn check_exhaustive(&mut self, span: SourceSpan, arms: &[MatchArm], metas: &[MatchArmMeta]) {
        let mut enum_name: Option<&str> = None;
        let mut covered: HashSet<&str> = HashSet::new();
        let mut mentioned: HashSet<&str> = HashSet::new();
        for (arm, meta) in arms.iter().zip(metas) {
            for alt in meta.pattern.alternatives() {
                let Pattern::Variant { name: tag, .. } = alt else {
                    return;
                };
                // Every variant pattern here is resolved: its name is the tag.
                let (Some(of_enum), variant) = crate::program::split_enum_tag(tag) else {
                    return;
                };
                if *enum_name.get_or_insert(of_enum) != of_enum {
                    return;
                }
                mentioned.insert(variant);
                if arm.guard.is_none() && covers_its_key(alt) {
                    covered.insert(variant);
                }
            }
        }
        let Some(enum_name) = enum_name else { return };
        let variants = self.enum_variant_names(enum_name);
        let missing: Vec<&String> = variants
            .iter()
            .filter(|v| !covered.contains(v.as_str()))
            .collect();
        if missing.is_empty() {
            return;
        }
        let names: Vec<String> = missing.iter().map(|v| format!("{enum_name}.{v}")).collect();
        // A variant the match does name, but only behind a condition.
        let partial = missing.iter().any(|v| mentioned.contains(v.as_str()));
        let note = if partial {
            " — an arm with an `if` guard, or whose payload patterns can fail, does not \
             cover its variant"
        } else {
            ""
        };
        self.error_at(
            span,
            format!(
                "this match does not cover every variant of `{enum_name}`: no arm for {}{note} \
                 (add the missing arm{}, or a `when _ ->` arm for everything else)",
                quoted_list(&names),
                if names.len() == 1 { "" } else { "s" },
            ),
        );
    }
}
