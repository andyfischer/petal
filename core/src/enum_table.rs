//! Enum tables: where a program's variants are numbered, and where a running
//! stack keeps what it knows about each one.
//!
//! A variant *value* is its tag (`"Kind.park"`, an interned heap string) and
//! its payload, and nothing else — that is what lets a variant kept in
//! `state` outlive the program that built it (docs/hot-reload.md, "Editing an
//! enum"), and what a host builds by hand with a tag no enum declares. What
//! the enum *declares* about the variant is looked up when it is read. This
//! module makes that lookup integer work:
//!
//! - [`EnumLayout`] numbers every declared variant when the program is
//!   lowered: a dense **slot**, enum by enum in declaration order, so one
//!   enum's variants are a contiguous run. The bytecode carries it
//!   ([`BytecodeProgram::enum_layout`]) with the slot of every string
//!   constant that is a variant's tag, so `Kind.park` is built from its slot.
//! - [`EnumTable`] is the stack's side: per slot, the tag's interned id and
//!   the constant-data record the run published; per enum, the `Kind.all`
//!   list. Tag ids are rooted here, so an id found in the table's index *is*
//!   that variant — a field read on a variant value is a probe of an integer
//!   hash and a slot load, with no string hashed or compared.
//!
//! A tag the index does not hold is resolved by its text, which is the old
//! path and still the right answer for the three values the index cannot
//! know: a variant a host built with a free-form tag, a variant of an enum
//! the program no longer declares, and a tag too long to be interned.
//!
//! [`BytecodeProgram::enum_layout`]: crate::backend::bytecode::BytecodeProgram

use std::collections::HashMap;
use std::sync::Arc;

use crate::heap::{Heap, ListId, MapId, StringId};
use crate::program::{Program, enum_variant_tag, split_enum_tag};
use crate::value::Value;

/// "No slot": the constant is not a declared variant's tag.
pub const NO_SLOT: u32 = u32::MAX;

/// One declared enum: its name and the run of slots its variants hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumShape {
    pub name: String,
    /// The slot of its first variant; the rest follow in declaration order.
    pub first: u32,
    pub len: u32,
}

/// One declared variant, at its slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantShape {
    /// The tag a value of this variant carries: `"Kind.park"`.
    pub tag: String,
    /// Payload field names, in constructor-argument order.
    pub fields: Vec<String>,
}

/// Every variant a program declares, numbered. A pure function of
/// [`Program::enums`], so two lowerings of one program (optimizer on and off)
/// agree on every slot.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EnumLayout {
    /// By name, sorted.
    pub enums: Vec<EnumShape>,
    /// By slot.
    pub variants: Vec<VariantShape>,
}

impl EnumLayout {
    pub fn of(program: &Program) -> Self {
        let mut layout = EnumLayout::default();
        // `Program::enums` is ordered by name, which `enum_index` relies on.
        for (name, def) in &program.enums {
            layout.enums.push(EnumShape {
                name: name.clone(),
                first: layout.variants.len() as u32,
                len: def.variants.len() as u32,
            });
            layout.variants.extend(def.variants.iter().map(|v| VariantShape {
                tag: enum_variant_tag(name, &v.name),
                fields: v.fields.clone(),
            }));
        }
        layout
    }

    /// The slot of every constant that is a declared variant's tag, by
    /// `ConstantId`; [`NO_SLOT`] for the rest.
    pub fn constant_slots(&self, constants: &crate::constant_table::ConstantTable) -> Vec<u32> {
        use crate::constant_table::ConstantValue;
        if self.variants.is_empty() {
            return Vec::new();
        }
        constants
            .values()
            .iter()
            .map(|c| match c {
                ConstantValue::String(s) => self.slot_of_text(s).unwrap_or(NO_SLOT),
                _ => NO_SLOT,
            })
            .collect()
    }

    /// The index of the enum named `name`.
    pub fn enum_index(&self, name: &str) -> Option<usize> {
        self.enums.binary_search_by(|e| e.name.as_str().cmp(name)).ok()
    }

    /// The slot of the variant whose tag reads `tag`, by its text.
    pub fn slot_of_text(&self, tag: &str) -> Option<u32> {
        let (enum_name, _) = split_enum_tag(tag);
        let shape = &self.enums[self.enum_index(enum_name?)?];
        (shape.first..shape.first + shape.len).find(|&slot| self.variants[slot as usize].tag == tag)
    }
}

/// An exact map from an interned string's id to a small number: open
/// addressing over a power-of-two table at most half full. Built once per
/// layout from ids the owner keeps alive, so an id it holds can never come to
/// name another string.
#[derive(Debug, Clone, Default)]
struct IdIndex {
    cells: Vec<Option<(StringId, u32)>>,
}

impl IdIndex {
    fn build(ids: &[StringId]) -> Self {
        if ids.is_empty() {
            return IdIndex::default();
        }
        let mut index = IdIndex {
            cells: vec![None; (ids.len() * 2).next_power_of_two().max(8)],
        };
        for (n, &id) in ids.iter().enumerate() {
            // The same id twice (two long tags cannot share one, and names
            // are unique) would be a layout bug; the first entry wins.
            if index.get(id).is_none() {
                let mut i = index.home(id);
                while index.cells[i].is_some() {
                    i = (i + 1) & (index.cells.len() - 1);
                }
                index.cells[i] = Some((id, n as u32));
            }
        }
        index
    }

    #[inline(always)]
    fn home(&self, id: StringId) -> usize {
        // Fibonacci hashing of the slot index: neighbouring ids, which is
        // what a run of freshly interned tags is, spread over the table.
        (id.index().wrapping_mul(0x9E37_79B1) >> 7) as usize & (self.cells.len() - 1)
    }

    #[inline(always)]
    fn get(&self, id: StringId) -> Option<u32> {
        if self.cells.is_empty() {
            return None;
        }
        let mask = self.cells.len() - 1;
        let mut i = self.home(id);
        loop {
            match self.cells[i] {
                Some((held, n)) if held == id => return Some(n),
                Some(_) => i = (i + 1) & mask,
                None => return None,
            }
        }
    }
}

/// What a running stack holds for the enums of the program it runs.
///
/// Rebuilt by [`sync`](Self::sync) when the program's layout changes and
/// emptied on hot reload ([`clear`](Self::clear)); the data records are
/// republished by every run, as the declarations re-execute.
#[derive(Debug, Clone, Default)]
pub struct EnumTable {
    layout: Option<Arc<EnumLayout>>,
    /// By slot: the interned id of the variant's tag.
    tags: Vec<StringId>,
    /// By slot: the record of constant data the last declaration published.
    records: Vec<Option<MapId>>,
    by_tag: IdIndex,
    /// By enum: the interned id of its name, and its `all` list once asked for.
    names: Vec<StringId>,
    alls: Vec<Option<ListId>>,
    by_name: IdIndex,
    /// Data published under a tag the layout does not declare. The compiler
    /// never does that; a hand-written IR document may.
    undeclared: HashMap<String, MapId>,
}

impl EnumTable {
    /// Make this table the one for `layout`, interning its tags in `heap`.
    /// Free when it already is, which is every call but the first after a
    /// load or a reload; a lowering of the same program under other flags has
    /// an equal layout and keeps everything.
    #[inline]
    pub fn sync(&mut self, layout: &Arc<EnumLayout>, heap: &mut Heap) {
        match &self.layout {
            Some(held) if Arc::ptr_eq(held, layout) => {}
            Some(held) if **held == **layout => self.layout = Some(layout.clone()),
            _ => self.rebuild(layout, heap),
        }
    }

    #[cold]
    fn rebuild(&mut self, layout: &Arc<EnumLayout>, heap: &mut Heap) {
        self.clear();
        self.tags = layout.variants.iter().map(|v| heap.intern_str(&v.tag)).collect();
        self.records = vec![None; layout.variants.len()];
        self.by_tag = IdIndex::build(&self.tags);
        self.names = layout.enums.iter().map(|e| heap.intern_str(&e.name)).collect();
        self.alls = vec![None; layout.enums.len()];
        self.by_name = IdIndex::build(&self.names);
        self.layout = Some(layout.clone());
    }

    /// Forget everything: the program this table described is gone.
    pub fn clear(&mut self) {
        *self = EnumTable::default();
    }

    /// The slot of the variant `tag` names, when the running program declares
    /// it and the id is the interned one. Integer work only.
    #[inline(always)]
    pub fn slot_of(&self, tag: StringId) -> Option<u32> {
        self.by_tag.get(tag)
    }

    /// [`slot_of`](Self::slot_of), falling back to the tag's text.
    pub fn resolve(&self, heap: &Heap, tag: StringId) -> Option<u32> {
        self.slot_of(tag)
            .or_else(|| self.layout.as_ref()?.slot_of_text(heap.get_string(tag)))
    }

    /// The declaration at `slot`.
    #[inline]
    pub fn shape(&self, slot: u32) -> Option<&VariantShape> {
        self.layout.as_ref()?.variants.get(slot as usize)
    }

    /// The interned tag of the variant at `slot`: what `Enum.variant` builds
    /// its value from, with no string interned at the construction.
    #[inline(always)]
    pub fn tag_at(&self, slot: u32) -> Option<StringId> {
        self.tags.get(slot as usize).copied()
    }

    /// The constant data published for the variant at `slot`.
    #[inline(always)]
    pub fn data_at(&self, slot: u32) -> Option<MapId> {
        self.records.get(slot as usize).copied().flatten()
    }

    /// The constant data of the variant `tag` names — the fast half of a
    /// field read: `None` sends the caller to the general path.
    #[inline(always)]
    pub fn data_of(&self, tag: StringId) -> Option<MapId> {
        self.data_at(self.slot_of(tag)?)
    }

    /// Data published under a tag no enum declares, by its text.
    pub fn undeclared_data(&self, tag: &str) -> Option<MapId> {
        self.undeclared.get(tag).copied()
    }

    /// Publish `record` as the constant data of the variant `tag` names;
    /// the record it replaces, if a run before published one.
    pub fn publish(&mut self, heap: &Heap, tag: StringId, record: MapId) -> Option<MapId> {
        match self.resolve(heap, tag) {
            Some(slot) => self.records[slot as usize].replace(record),
            None => self.undeclared.insert(heap.get_string(tag).to_string(), record),
        }
    }

    /// [`publish`](Self::publish) for the usual case, where it changes
    /// nothing a reader could see: `record` holds, value for value, what the
    /// record already published for this variant holds. The new record takes
    /// the slot (the old one is garbage now) and `true` says the caller has
    /// nothing more to do; `false` leaves everything to `publish` and the
    /// caller's own comparison.
    ///
    /// "Value for value" is identity for anything on the heap, so a `true`
    /// here is always equal content; data that builds an equal list afresh
    /// each run is `false`, and is compared in depth by the caller.
    #[inline]
    pub fn republish(&mut self, heap: &Heap, tag: StringId, record: MapId) -> bool {
        let Some(slot) = self.slot_of(tag) else {
            return false;
        };
        let Some(old) = self.records[slot as usize] else {
            return false;
        };
        if old != record {
            let (was, now) = (heap.get_map(old), heap.get_map(record));
            if was.shape_id() != now.shape_id()
                || was.values_slice() != now.values_slice()
                || heap.map_class_name(old) != heap.map_class_name(record)
            {
                return false;
            }
        }
        self.records[slot as usize] = Some(record);
        true
    }

    /// `Enum.all` for the enum `name` names: its payload-free variants in
    /// declaration order. Built on the first ask and handed out again after:
    /// every reader holds the *same* list, so it must never be written
    /// through — which holds because nothing treats the result of the builtin
    /// behind `Enum.all` as a container it owns (it is not on
    /// [`crate::builtins::returns_fresh_container`]'s list, and a `var` cell
    /// does not own a value it was handed).
    pub fn all(&mut self, heap: &mut Heap, name: StringId) -> ListId {
        let found = self.by_name.get(name).map(|i| i as usize).or_else(|| {
            self.layout.as_ref()?.enum_index(heap.get_string(name))
        });
        let (Some(ix), Some(layout)) = (found, self.layout.as_ref()) else {
            return heap.alloc_list(Vec::new());
        };
        if let Some(list) = self.alls[ix] {
            return list;
        }
        let shape = &layout.enums[ix];
        let data = heap.empty_payload();
        let variants = (shape.first..shape.first + shape.len)
            .filter(|&slot| layout.variants[slot as usize].fields.is_empty())
            .map(|slot| Value::EnumVariant {
                tag: self.tags[slot as usize],
                data,
            })
            .collect();
        let list = heap.alloc_list(variants);
        self.alls[ix] = Some(list);
        list
    }

    /// [`all`](Self::all) when the list is already built and `name` is the
    /// enum's interned name: a read, for the VM's straight-line loop.
    #[inline(always)]
    pub fn all_cached(&self, name: StringId) -> Option<ListId> {
        self.alls.get(self.by_name.get(name)? as usize).copied().flatten()
    }

    /// Everything here that must survive a collection: the tag and name
    /// strings the indexes are keyed by, the published records and the
    /// `all` lists.
    pub fn gc_roots(&self, mut mark: impl FnMut(Value)) {
        for &id in self.tags.iter().chain(&self.names) {
            mark(Value::String(id));
        }
        for &rec in self.records.iter().flatten().chain(self.undeclared.values()) {
            mark(Value::Map(rec));
        }
        for &list in self.alls.iter().flatten() {
            mark(Value::List(list));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_index_is_exact() {
        let mut heap = Heap::new();
        let ids: Vec<StringId> = (0..300).map(|i| heap.intern_str(&format!("K.v{i}"))).collect();
        let index = IdIndex::build(&ids);
        for (n, &id) in ids.iter().enumerate() {
            assert_eq!(index.get(id), Some(n as u32));
        }
        let other = heap.intern_str("K.other");
        assert_eq!(index.get(other), None);
        assert_eq!(IdIndex::default().get(other), None);
    }
}
