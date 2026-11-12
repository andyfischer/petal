//! Record templates: the binding-time split of a record literal (L6 of the
//! retained scene work, `docs/dev/retained-scene-output.md`).
//!
//! In `body(id, {shape: "box", size: 2.0, mass: 5.0, color: :f80, pos: p})`
//! four of the five fields are the same on every call. The lowering marks
//! such a literal (an `Inst::AllocMap` whose fields are mostly static) with a
//! [`TemplatePlan`]: which fields are *holes* (vary per record) and the
//! literal's template *site*. At run time each stack keeps, per site, the
//! static fields' values as the first record built there had them, and a
//! [`Template`] naming them: a content id that is the same for the same keys,
//! hole positions and static values in any run, after a reload and in another
//! process. Every record the site builds whose static fields still hold
//! those values carries the template ([`RecordMap::template`]), so a host
//! that has decoded one record of the template decodes only the holes of
//! the next (the C bridge does: `pb_value::tmpl`), and a keyed declaration
//! whose template is the one it applied last compares only its holes.
//!
//! # Which literals
//!
//! A plain record literal (not a class instance, no repeated key) with at
//! least as many static fields as holes. A field is static when its value
//! is a constant, a top-level value the static-value cache may keep
//! ([`StaticPlan::roots`]), or a term inside a function body that is
//! invariant across calls ([`FnPlan::invariant`], the expression-level
//! classifier the cache keeps function-body values by). So the pass runs
//! with the static-value cache, after it.
//!
//! # Invisible to the program
//!
//! The record is built exactly as without the template, from the same
//! registers; the template is a tag beside its values that nothing in the
//! language reads. Equality, printing, field reads, spreads and copies do
//! not look at it, and any write to the record drops it.
//!
//! # Not trusted: checked
//!
//! The classifier picks the holes; it does not vouch for the tag. Before a
//! record is tagged, its static fields are compared with the site's kept
//! values: the same heap object (the static-value cache hands every call
//! the same one) costs a compare, an equal one is compared by contents
//! ([`identical`]). When one differs the site starts over from that record
//! under a new id (a `config let` the host changed, a literal a reload
//! patched); a site that keeps differing
//! ([`MAX_REBUILDS`]) stops tagging. The kept values are made immutable, as
//! the cache's are, so the contents a template id names cannot change under
//! it. A value with no contents to name (a function, a cell, a handle, a
//! pending result) rejects the site.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::backend::bytecode::isa::{BytecodeProgram, Inst};
use crate::heap::Heap;
use crate::program::{Program, TermId, TermOp};
use crate::record::{Shape, Template};
use crate::static_cache::FnPlan;
use crate::static_cache::{StaticPlan, identical};
use crate::value::Value;

/// What the lowering decided for one templated literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplatePlan {
    /// The stack's site table index.
    pub site: u32,
    /// Field positions that are not static, ascending.
    pub holes: Box<[u32]>,
    /// The other field positions, ascending.
    pub statics: Box<[u32]>,
    /// The static fields compared at run time, as indexes into `statics`:
    /// the ones that are not a constant of the program. A constant is the
    /// same value on every evaluation by construction (a patched literal
    /// empties the sites).
    pub checked: Box<[u32]>,
}

impl TemplatePlan {
    /// The plan for a literal of `len` fields with these holes, whose
    /// fields `constant` are constants of the program.
    pub fn new(site: u32, len: usize, holes: Vec<u32>, constant: &[u32]) -> TemplatePlan {
        let statics: Box<[u32]> = (0..len as u32).filter(|i| !holes.contains(i)).collect();
        let checked = (0..statics.len() as u32)
            .filter(|&k| !constant.contains(&statics[k as usize]))
            .collect();
        TemplatePlan {
            site,
            holes: holes.into_boxed_slice(),
            statics,
            checked,
        }
    }
}

/// A site that started over this many times stops tagging records.
pub const MAX_REBUILDS: u32 = 8;

#[derive(Debug, Clone, Default)]
enum Site {
    #[default]
    Empty,
    Ready {
        /// The static fields' values, in field order (holes skipped).
        statics: Box<[Value]>,
        template: Arc<Template>,
        rebuilds: u32,
    },
    /// Values a template cannot name, or too many rebuilds.
    Rejected,
}

/// Counters of one stack's sites, cumulative.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TemplateStats {
    /// Records built with a template.
    pub tagged: u64,
    /// Records whose static fields differed from the site's: the site
    /// started over from the record (or gave up).
    pub rebuilt: u64,
    /// Sites built (first records, and starts over).
    pub built: u64,
    /// Sites rejected.
    pub rejected: u64,
}

/// One stack's template sites (kept beside its static values, and emptied
/// with them).
#[derive(Debug, Clone, Default)]
pub struct TemplateSites {
    sites: Vec<Site>,
    /// Records are not tagged at all ([`set_enabled`](Self::set_enabled)).
    off: bool,
    pub stats: TemplateStats,
}

impl TemplateSites {
    /// Make room for the `n` sites of a lowering, all empty.
    pub(crate) fn reset(&mut self, n: usize) {
        self.sites.clear();
        self.sites.resize(n, Site::Empty);
    }

    pub(crate) fn clear(&mut self) {
        self.sites.clear();
    }

    /// Tag records or not. Off, every literal builds untagged records, as
    /// without the pass: the oracle the tags are held to.
    #[inline]
    pub fn set_enabled(&mut self, on: bool) {
        if self.off == on {
            self.off = !on;
            self.sites.iter_mut().for_each(|s| *s = Site::Empty);
        }
    }

    pub fn enabled(&self) -> bool {
        !self.off
    }

    /// The template for a record `plan`'s literal is building from
    /// `fields` (its values in field order), or `None` to build it untagged.
    #[inline]
    pub(crate) fn check(
        &mut self,
        plan: &TemplatePlan,
        shape: &Arc<Shape>,
        fields: &[Value],
        heap: &mut Heap,
    ) -> Option<Arc<Template>> {
        if self.off {
            return None;
        }
        let site = self.sites.get_mut(plan.site as usize)?;
        let rebuilds = match site {
            Site::Rejected => return None,
            Site::Ready {
                statics,
                template,
                rebuilds,
            } => {
                if same_statics(statics, plan, fields, heap) {
                    self.stats.tagged += 1;
                    return Some(template.clone());
                }
                self.stats.rebuilt += 1;
                *rebuilds + 1
            }
            Site::Empty => 0,
        };
        if rebuilds > MAX_REBUILDS {
            *site = Site::Rejected;
            self.stats.rejected += 1;
            return None;
        }
        // Build the site from this record. Its static values are made
        // immutable (as a kept static value is) so the contents the id
        // names cannot change under it.
        let Some(statics) = plan
            .statics
            .iter()
            .map(|&i| fields.get(i as usize).copied())
            .collect::<Option<Vec<Value>>>()
        else {
            return None;
        };
        for v in &statics {
            if heap.mark_immutable(*v).is_err() {
                *site = Site::Rejected;
                self.stats.rejected += 1;
                return None;
            }
        }
        let Some(id) = content_id(shape, &plan.holes, &statics, heap) else {
            *site = Site::Rejected;
            self.stats.rejected += 1;
            return None;
        };
        let template = Arc::new(Template {
            id,
            holes: plan.holes.clone(),
            len: fields.len() as u32,
        });
        *site = Site::Ready {
            statics: statics.into_boxed_slice(),
            template: template.clone(),
            rebuilds,
        };
        self.stats.built += 1;
        self.stats.tagged += 1;
        Some(template)
    }

    pub(crate) fn gc_roots(&self, mut mark: impl FnMut(Value)) {
        for s in &self.sites {
            if let Site::Ready { statics, .. } = s {
                for v in statics.iter() {
                    mark(*v);
                }
            }
        }
    }
}

/// Whether the static fields of `fields` that `plan` checks are `kept`'s
/// (which holds every static field), value for value.
#[inline]
fn same_statics(kept: &[Value], plan: &TemplatePlan, fields: &[Value], heap: &Heap) -> bool {
    plan.checked.iter().all(|&k| {
        let k = k as usize;
        match (
            kept.get(k),
            plan.statics.get(k).and_then(|&i| fields.get(i as usize)),
        ) {
            (Some(old), Some(v)) => bits_eq(*old, *v) || identical(*old, *v, heap),
            _ => false,
        }
    })
}

/// The same scalar bit for bit, or the same heap object.
#[inline]
fn bits_eq(a: Value, b: Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Nil, Value::Nil) => true,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Vec3(x), Value::Vec3(y)) => x == y,
        (Value::List(x), Value::List(y)) => x == y,
        (Value::Map(x), Value::Map(y)) => x == y,
        _ => false,
    }
}

/// The id of a template: a hash of the keys, the hole positions and the
/// static values' exact contents (numbers by bits and kind, so `1` and
/// `1.0` differ; strings by bytes; containers element by element, records
/// with their keys and class). SipHash with fixed keys, so the same in
/// every process built from the same Petal (std's `DefaultHasher` may change
/// between Rust releases; a host must not keep ids across builds). A symbol
/// is hashed by its id, so only within one process. `None` when a value has
/// no contents to name.
pub fn content_id(shape: &Shape, holes: &[u32], statics: &[Value], heap: &Heap) -> Option<u64> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    shape.len().hash(&mut h);
    for k in shape.keys() {
        k.as_str().hash(&mut h);
    }
    holes.hash(&mut h);
    for v in statics {
        hash_exact(*v, heap, &mut h)?;
    }
    // Never 0: a host reads 0 as "no template".
    Some(h.finish().max(1))
}

fn hash_exact(v: Value, heap: &Heap, h: &mut impl Hasher) -> Option<()> {
    match v {
        Value::Nil => 0u8.hash(h),
        Value::Bool(b) => {
            1u8.hash(h);
            b.hash(h);
        }
        Value::Int(i) => {
            2u8.hash(h);
            i.hash(h);
        }
        Value::Float(f) => {
            3u8.hash(h);
            f.to_bits().hash(h);
        }
        Value::String(id) => {
            4u8.hash(h);
            heap.get_string(id).hash(h);
        }
        Value::List(id) => {
            5u8.hash(h);
            let xs = heap.get_list(id);
            xs.len().hash(h);
            for x in xs {
                hash_exact(*x, heap, h)?;
            }
        }
        Value::Vec2(id) => {
            6u8.hash(h);
            let (x, y) = heap.get_vec2(id);
            x.to_bits().hash(h);
            y.to_bits().hash(h);
        }
        Value::Vec3(id) => {
            7u8.hash(h);
            for c in heap.get_vec3(id) {
                c.to_bits().hash(h);
            }
        }
        Value::F64Array(id) => {
            8u8.hash(h);
            let xs = heap.get_f64_array(id);
            xs.len().hash(h);
            for x in xs {
                x.to_bits().hash(h);
            }
        }
        Value::Map(id) => {
            9u8.hash(h);
            heap.map_class_name(id).hash(h);
            let m = heap.get_map(id);
            m.len().hash(h);
            for (k, x) in m.iter() {
                k.as_str().hash(h);
                hash_exact(*x, heap, h)?;
            }
        }
        // By its id in the VM's symbol table: stable within a process.
        Value::Symbol(s) => {
            10u8.hash(h);
            s.0.hash(h);
        }
        Value::EnumVariant { tag, data } => {
            11u8.hash(h);
            heap.tag_str(tag).hash(h);
            hash_exact(Value::List(data), heap, h)?;
        }
        Value::Dict(id) => {
            13u8.hash(h);
            let d = heap.get_dict(id);
            d.len().hash(h);
            for (k, x) in d.iter() {
                hash_exact(k, heap, h)?;
                hash_exact(x, heap, h)?;
            }
        }
        _ => return None,
    }
    Some(())
}

// ---------------------------------------------------------------------------
// The lowering pass
// ---------------------------------------------------------------------------

/// Mark the templated literals of `bc` (see the module docs). Returns the
/// number of sites. `fn_plan` is the classifier's invariant terms inside
/// function bodies, when the cache keeps those.
pub fn apply(
    bc: &mut BytecodeProgram,
    program: &Program,
    plan: &StaticPlan,
    fn_plan: Option<&FnPlan>,
) -> u32 {
    let mut sites = 0u32;
    let fns = std::iter::once(&mut bc.root).chain(bc.fns.iter_mut());
    for f in fns {
        for (inst, origin) in f.code.iter_mut().zip(f.origins.iter()) {
            let Inst::AllocMap {
                vals,
                class: None,
                shape: Some(shape),
                template,
                ..
            } = inst
            else {
                continue;
            };
            let Some(t) = origin else { continue };
            let Some(term) = program.terms.get(t.0 as usize) else {
                continue;
            };
            if !matches!(term.op, TermOp::AllocMap { class: None, .. })
                || term.inputs.len() != vals.len()
                || shape.len() != vals.len()
                || vals.is_empty()
            {
                continue;
            }
            let mut holes: Vec<u32> = vec![];
            let mut constant: Vec<u32> = vec![];
            for (j, i) in term.inputs.iter().enumerate() {
                match field_class(program, plan, fn_plan, *i) {
                    Field::Constant => constant.push(j as u32),
                    Field::Static => {}
                    Field::Hole => holes.push(j as u32),
                }
            }
            let statics = vals.len() - holes.len();
            if statics == 0 || statics < holes.len() {
                continue;
            }
            *template = Some(Arc::new(TemplatePlan::new(
                sites,
                vals.len(),
                holes,
                &constant,
            )));
            sites += 1;
        }
    }
    sites
}

enum Field {
    /// A constant of the program.
    Constant,
    /// The same every time, by the classifier.
    Static,
    Hole,
}

/// Whether the value of term `t` is the same every time the literal reading
/// it is evaluated.
fn field_class(program: &Program, plan: &StaticPlan, fn_plan: Option<&FnPlan>, t: TermId) -> Field {
    let mut t = t;
    // A variable read is its source.
    for _ in 0..64 {
        let Some(term) = program.terms.get(t.0 as usize) else {
            return Field::Hole;
        };
        if matches!(term.op, TermOp::Constant(_)) && term.inputs.is_empty() {
            return Field::Constant;
        }
        let stat = |yes: bool| if yes { Field::Static } else { Field::Hole };
        if term.block_id == program.root_block {
            return stat(plan.roots.contains(&t));
        }
        if let Some(fp) = fn_plan
            && fp.function_of(t).is_some()
        {
            return stat(fp.invariant(t));
        }
        match (&term.op, term.inputs.as_slice()) {
            (TermOp::Copy, [src]) => t = *src,
            _ => return Field::Hole,
        }
    }
    Field::Hole
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Key;

    fn shape(keys: &[&str]) -> Arc<Shape> {
        Shape::from_keys(keys.iter().map(|k| Key::new(k))).unwrap()
    }

    fn sites(n: usize) -> TemplateSites {
        let mut s = TemplateSites::default();
        s.reset(n);
        s
    }

    #[test]
    fn a_site_tags_records_whose_static_fields_are_its_own() {
        let mut heap = Heap::new();
        let sh = shape(&["shape", "size", "pos"]);
        let plan = TemplatePlan::new(0, 3, vec![2], &[]);
        let mut s = sites(1);
        let box_ = Value::String(heap.alloc_string("box".into()));
        let a = s
            .check(
                &plan,
                &sh,
                &[box_, Value::Float(2.0), Value::Int(1)],
                &mut heap,
            )
            .unwrap();
        // The hole differs: the same template.
        let b = s
            .check(
                &plan,
                &sh,
                &[box_, Value::Float(2.0), Value::Int(9)],
                &mut heap,
            )
            .unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        // An equal string that is another object: compared by contents.
        let box2 = Value::String(heap.alloc_string("box".into()));
        let c = s
            .check(
                &plan,
                &sh,
                &[box2, Value::Float(2.0), Value::Int(9)],
                &mut heap,
            )
            .unwrap();
        assert!(Arc::ptr_eq(&a, &c));
        assert_eq!(s.stats.tagged, 3);
        assert_eq!(&*a.holes, &[2]);
        assert_eq!(a.len, 3);
    }

    #[test]
    fn a_static_field_that_changes_starts_the_site_over_under_another_id() {
        let mut heap = Heap::new();
        let sh = shape(&["size", "pos"]);
        let plan = TemplatePlan::new(0, 2, vec![1], &[]);
        let mut s = sites(1);
        let a = s
            .check(&plan, &sh, &[Value::Float(2.0), Value::Int(1)], &mut heap)
            .unwrap();
        let b = s
            .check(&plan, &sh, &[Value::Float(3.0), Value::Int(1)], &mut heap)
            .unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(s.stats.rebuilt, 1);
        // `2` is not `2.0`.
        let c = s
            .check(&plan, &sh, &[Value::Int(2), Value::Int(1)], &mut heap)
            .unwrap();
        assert_ne!(a.id, c.id);
        assert_ne!(b.id, c.id);
        // Back to the first values: the first id again.
        let d = s
            .check(&plan, &sh, &[Value::Float(2.0), Value::Int(1)], &mut heap)
            .unwrap();
        assert_eq!(a.id, d.id);
        // A site that keeps changing gives up.
        for i in 0..MAX_REBUILDS {
            s.check(
                &plan,
                &sh,
                &[Value::Int(100 + i as i64), Value::Int(1)],
                &mut heap,
            );
        }
        assert!(
            s.check(&plan, &sh, &[Value::Int(7), Value::Int(1)], &mut heap)
                .is_none()
        );
        assert_eq!(s.stats.rejected, 1);
        assert!(
            s.check(&plan, &sh, &[Value::Int(7), Value::Int(1)], &mut heap)
                .is_none()
        );
    }

    #[test]
    fn kept_values_are_made_immutable_and_a_pending_value_rejects_the_site() {
        let mut heap = Heap::new();
        let sh = shape(&["tint", "pos"]);
        let plan = TemplatePlan::new(0, 2, vec![1], &[]);
        let mut s = sites(2);
        let l = Value::List(heap.alloc_list(vec![Value::Float(1.0)]));
        assert!(
            s.check(&plan, &sh, &[l, Value::Int(1)], &mut heap)
                .is_some()
        );
        assert!(heap.is_immutable(l));
        let plan2 = TemplatePlan::new(1, 2, vec![1], &[]);
        let f = Value::Pending(crate::value::PendingId(0));
        assert!(
            s.check(&plan2, &sh, &[f, Value::Int(1)], &mut heap)
                .is_none()
        );
        assert_eq!(s.stats.rejected, 1);
    }

    #[test]
    fn off_tags_nothing() {
        let mut heap = Heap::new();
        let sh = shape(&["size", "pos"]);
        let plan = TemplatePlan::new(0, 2, vec![1], &[]);
        let mut s = sites(1);
        s.set_enabled(false);
        assert!(
            s.check(&plan, &sh, &[Value::Float(2.0), Value::Int(1)], &mut heap)
                .is_none()
        );
        s.set_enabled(true);
        assert!(
            s.check(&plan, &sh, &[Value::Float(2.0), Value::Int(1)], &mut heap)
                .is_some()
        );
    }
}
