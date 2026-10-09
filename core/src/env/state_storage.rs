//! State storage: a stack's whole `state` as one JSON document that a later
//! process can load back, which is what `petal run --state-storage <file>`
//! reads before a run and writes after it.
//!
//! This is a different surface from [`Env::get_state_json`] /
//! [`Env::set_state_map_from_json`] (`state_json.rs`). Those are a host's
//! *inspection* view: keyed by display name, top-level slots only on the way
//! back in, and lossy in the values (an enum comes back as a record, a
//! closure as its display string). A storage document has to give the next
//! run the state this run ended with, so it differs in two ways:
//!
//! - **Every slot is addressed.** A slot is saved under its declaration id and
//!   call path ([`RuntimeStateKey`]), so state declared inside a function, a
//!   loop or a `state(key)` group comes back too. Both halves are derived
//!   from names (`compiler::state_ids`), so they are the same in every
//!   process that compiles the same source.
//! - **Values round-trip or are not saved.** Anything JSON has no spelling
//!   for is written as an object tagged `"$petal"` (see [`encode`]); a value
//!   that cannot be rebuilt in another process — a function, a host handle,
//!   a pending resource, a UI element — is reported and left out, so its
//!   declaration starts from its initial value next run instead of coming
//!   back as a string.
//!
//! The document:
//!
//! ```json
//! {
//!   "format": "petal-state",
//!   "version": 1,
//!   "slots": [
//!     { "name": "hits", "key": "5033…", "value": 2 },
//!     { "name": "counter/count", "key": "9120…",
//!       "path": [{ "call": "7781…" }, { "index": 3 }], "value": 1 }
//!   ]
//! }
//! ```
//!
//! `name` is for the reader. `key` and the ids inside `path` are `u64`s
//! written as decimal strings, since a JSON number cannot hold one exactly. A
//! top-level slot (no `path`) may omit `key`, and is then matched by `name` —
//! which is what makes a hand-written file work.

use std::collections::HashSet;

use serde_json::{Value as Json, json};
use smallvec::SmallVec;

use super::state_json::{call_site_labels, render_state_key};
use super::*;
use crate::program::TermOp;
use crate::stack::PathPart;
use crate::symbol::SymbolTable;

/// The `"format"` value that marks a state storage document.
pub const STATE_STORAGE_FORMAT: &str = "petal-state";
/// The document version this build writes, and the only one it reads.
pub const STATE_STORAGE_VERSION: u64 = 1;

/// The key that marks an object as an encoded non-JSON value rather than a
/// record. `$` cannot start a Petal identifier, so only a record built with a
/// string key (`r["$petal"] = …`) can collide, and [`encode`] escapes that.
const TAG: &str = "$petal";

/// What [`Env::save_state_storage`] produced.
pub struct StateStorageSave {
    /// The document to write.
    pub document: Json,
    /// How many slots it holds.
    pub saved: usize,
    /// Slots left out because their value cannot be stored: the slot's display
    /// name and what it held ("a function").
    pub skipped: Vec<(String, String)>,
}

/// What [`Env::load_state_storage`] did with a document.
pub struct StateStorageLoad {
    /// How many slots were put into the stack.
    pub restored: usize,
    /// Display names of the slots the program has no declaration for. They are
    /// not loaded, so the next save drops them.
    pub dropped: Vec<String>,
}

impl Env {
    /// The names of the program's `state` declarations, in source order and
    /// without repeats. Module state is module-qualified (`ui::theme`).
    pub fn declared_state_names(&self, program_id: ProgramId) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut names = Vec::new();
        if let Some(program) = self.programs.get(&program_id) {
            for term in &program.terms {
                if matches!(term.op, TermOp::StateInit)
                    && let Some(name) = &term.name
                    && seen.insert(name.as_str())
                {
                    names.push(name.clone());
                }
            }
        }
        names
    }

    /// Serialize every state slot of `stack_id` as a storage document (see the
    /// module docs). Slots whose value cannot be stored are listed in
    /// [`StateStorageSave::skipped`] rather than written.
    pub fn save_state_storage(&self, program_id: ProgramId, stack_id: StackKey) -> StateStorageSave {
        let names = self.state_key_names(program_id);
        let ck = self.ctx_for(stack_id).unwrap_or(self.default_context);
        let heap = &self.ctx(ck).heap;
        let labels = match self.get_program(program_id) {
            Some(program) => call_site_labels(program),
            None => HashMap::new(),
        };

        let mut slots: Vec<(String, u64, Json)> = Vec::new();
        let mut skipped = Vec::new();
        if let Some(state) = self.get_all_state(stack_id) {
            for (key, val) in state {
                let base_name = names
                    .get(&key.base)
                    .cloned()
                    .unwrap_or_else(|| format!("unknown_{}", key.base.0));
                let name = render_state_key(&base_name, &key.path, &labels);
                // A `state var` slot holds its cell; what is stored is the
                // contents, and the load side makes a new cell for them.
                let val = match val {
                    Value::Cell(cell) => heap.cell_read(*cell),
                    other => *other,
                };
                match encode(&val, heap, &self.symbols) {
                    Ok(value) => {
                        let mut slot = serde_json::Map::new();
                        slot.insert("name".to_string(), json!(name));
                        slot.insert("key".to_string(), json!(key.base.0.to_string()));
                        if !key.path.is_empty() {
                            let path: Vec<Json> = key.path.iter().map(path_part_json).collect();
                            slot.insert("path".to_string(), Json::Array(path));
                        }
                        slot.insert("value".to_string(), value);
                        slots.push((name, key.base.0, Json::Object(slot)));
                    }
                    Err(what) => skipped.push((name, what)),
                }
            }
        }
        // The state map is a hash map: sort, so two saves of the same state
        // are the same bytes. The path breaks ties between slots that render
        // to one name.
        slots.sort_by_cached_key(|(name, key, slot)| (name.clone(), *key, slot["path"].to_string()));
        skipped.sort();
        let saved = slots.len();
        let slots: Vec<Json> = slots.into_iter().map(|(_, _, slot)| slot).collect();
        StateStorageSave {
            document: json!({
                "format": STATE_STORAGE_FORMAT,
                "version": STATE_STORAGE_VERSION,
                "slots": slots,
            }),
            saved,
            skipped,
        }
    }

    /// Put a storage document's slots into `stack_id`, before its first run.
    ///
    /// An error means the document is not usable and nothing was loaded: it is
    /// not a storage document, it is a version this build does not read, a
    /// slot is malformed, or it holds slots and *none* of them belongs to this
    /// program (it was written by a different script). A document where only
    /// some slots have no declaration loads the rest and names the others in
    /// [`StateStorageLoad::dropped`].
    pub fn load_state_storage(
        &mut self,
        program_id: ProgramId,
        stack_id: StackKey,
        document: &Json,
    ) -> Result<StateStorageLoad, String> {
        let doc = document
            .as_object()
            .ok_or("it is not a Petal state storage file (expected a JSON object)")?;
        match doc.get("format").and_then(Json::as_str) {
            Some(STATE_STORAGE_FORMAT) => {}
            _ => {
                return Err(format!(
                    "it is not a Petal state storage file (expected \"format\": \
                     \"{STATE_STORAGE_FORMAT}\")"
                ));
            }
        }
        match doc.get("version") {
            Some(v) if v.as_u64() == Some(STATE_STORAGE_VERSION) => {}
            Some(v) => {
                return Err(format!(
                    "it is state storage version {v}, and this petal reads version \
                     {STATE_STORAGE_VERSION}"
                ));
            }
            None => return Err("it has no \"version\"".to_string()),
        }
        let slots = doc
            .get("slots")
            .and_then(Json::as_array)
            .ok_or("it has no \"slots\" list")?;

        // What the program declares: every declaration id, the ones that are
        // `state var` (their slot holds a cell), and the id behind each name.
        let program = self.programs.get(&program_id).ok_or("Program not found")?;
        let mut declared: HashSet<StateKey> = HashSet::new();
        let mut cells: HashSet<StateKey> = HashSet::new();
        let mut by_name: HashMap<&str, StateKey> = HashMap::new();
        for term in &program.terms {
            let Some(key) = term.state_key else { continue };
            declared.insert(key);
            if !matches!(term.op, TermOp::StateInit) {
                continue;
            }
            if let Some(name) = &term.name {
                by_name.entry(name.as_str()).or_insert(key);
            }
            // `state var` ends its init block with the `CellNew` that boxes
            // the initial value (`Compiler::compile_state_decl`).
            let boxed = term
                .child_blocks
                .first()
                .and_then(|b| program.get_block(*b).terms.last())
                .is_some_and(|t| matches!(program.get_term(*t).op, TermOp::CellNew));
            if boxed {
                cells.insert(key);
            }
        }

        let ck = self.ctx_for(stack_id).ok_or("Stack not found")?;
        let mut restored: Vec<(RuntimeStateKey, Value)> = Vec::new();
        let mut dropped = Vec::new();
        for (i, slot) in slots.iter().enumerate() {
            let n = i + 1;
            let slot = slot
                .as_object()
                .ok_or_else(|| format!("slot {n} is not an object"))?;
            let name = match slot.get("name") {
                None => None,
                Some(Json::String(s)) => Some(s.as_str()),
                Some(_) => return Err(format!("slot {n}: \"name\" is not a string")),
            };
            let label = match name {
                Some(name) => format!("`{name}`"),
                None => format!("slot {n}"),
            };
            let path = match slot.get("path") {
                None => SmallVec::new(),
                Some(Json::Array(parts)) => parts
                    .iter()
                    .map(path_part_from_json)
                    .collect::<Result<SmallVec<[PathPart; 4]>, String>>()
                    .map_err(|e| format!("{label}: {e}"))?,
                Some(_) => return Err(format!("{label}: \"path\" is not a list")),
            };
            let key = match slot.get("key") {
                None => None,
                Some(k) => Some(StateKey(
                    parse_id(k).map_err(|e| format!("{label}: \"key\" {e}"))?,
                )),
            };
            if key.is_none() && name.is_none() {
                return Err(format!("slot {n} has neither a \"name\" nor a \"key\""));
            }
            if key.is_none() && !path.is_empty() {
                return Err(format!("{label}: a slot with a \"path\" needs a \"key\""));
            }
            let value = slot
                .get("value")
                .ok_or_else(|| format!("{label} has no \"value\""))?;

            // The key decides; a top-level slot falls back to its name, so a
            // hand-written `{"name": "hits", "value": 5}` finds its slot.
            let base = key.filter(|k| declared.contains(k)).or_else(|| {
                path.is_empty()
                    .then(|| name.and_then(|n| by_name.get(n).copied()))
                    .flatten()
            });
            let Some(base) = base else {
                dropped.push(name.map(str::to_string).unwrap_or(label));
                continue;
            };

            let ctx = self.contexts.get_mut(&ck).expect("context exists");
            let mut val = decode(value, &mut ctx.heap, &mut self.symbols)
                .map_err(|e| format!("{label}: {e}"))?;
            if cells.contains(&base) {
                val = Value::Cell(ctx.heap.alloc_cell(val));
            }
            restored.push((RuntimeStateKey { base, path }, val));
        }

        if restored.is_empty() && !dropped.is_empty() {
            return Err(format!(
                "none of the state saved in it ({}) is declared by this script, so it was \
                 written by a different script",
                name_list(&dropped)
            ));
        }

        let count = restored.len();
        let stack = self.stacks.get_mut(&stack_id).ok_or("Stack not found")?;
        stack.state.extend(restored);
        // State put in from outside is a change no run recorded.
        stack.run_deps.force();
        Ok(StateStorageLoad {
            restored: count,
            dropped,
        })
    }
}

/// Up to four backquoted names, then a count of the rest.
pub fn name_list(names: &[String]) -> String {
    const SHOWN: usize = 4;
    let mut out = names
        .iter()
        .take(SHOWN)
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > SHOWN {
        out.push_str(&format!(" and {} more", names.len() - SHOWN));
    }
    out
}

fn path_part_json(part: &PathPart) -> Json {
    match part {
        PathPart::Call(h) => json!({ "call": h.to_string() }),
        PathPart::Index(i) => json!({ "index": i }),
        PathPart::Key(h) => json!({ "key": h.to_string() }),
    }
}

fn path_part_from_json(part: &Json) -> Result<PathPart, String> {
    let bad = || format!("unreadable \"path\" step {part}");
    let obj = part.as_object().filter(|o| o.len() == 1).ok_or_else(bad)?;
    let (kind, v) = obj.iter().next().ok_or_else(bad)?;
    match kind.as_str() {
        "call" => Ok(PathPart::Call(parse_id(v).map_err(|_| bad())?)),
        "key" => Ok(PathPart::Key(parse_id(v).map_err(|_| bad())?)),
        "index" => v
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .map(PathPart::Index)
            .ok_or_else(bad),
        _ => Err(bad()),
    }
}

/// A `u64` id written as a decimal string (a plain JSON number is accepted
/// too, for a hand-written file).
fn parse_id(v: &Json) -> Result<u64, String> {
    match v {
        Json::String(s) => s.parse::<u64>().ok(),
        other => other.as_u64(),
    }
    .ok_or_else(|| format!("is not an id (a decimal string): {v}"))
}

/// Encode a value for storage, or say what it is when it cannot be stored.
///
/// `nil`, booleans, ints, finite floats, strings and lists are themselves.
/// A record is a JSON object when its fields are in sorted order and none is
/// named `$petal`; otherwise — and for everything else — the value is an
/// object tagged `"$petal"`:
///
/// | Value | Encoding |
/// |-------|----------|
/// | `nan`, `inf`, `-inf` | `{"$petal": "float", "value": "nan"}` |
/// | record, any field order | `{"$petal": "record", "fields": [[name, value], …]}` |
/// | class instance | `{"$petal": "instance", "class": name, "fields": […]}` |
/// | enum variant | `{"$petal": "enum", "tag": name, "data": […]}` |
/// | f64 array | `{"$petal": "f64_array", "values": […]}` |
/// | `vec2` / `vec3` | `{"$petal": "vec2", "x": …, "y": …}` |
/// | dual number | `{"$petal": "dual", "value": …, "derivative": …}` |
/// | symbol | `{"$petal": "symbol", "name": …}` |
///
/// The ordered record form exists because a JSON object here is a sorted map:
/// writing `{b: 1, a: 2}` as one would bring it back as `{a: 2, b: 1}`, and
/// field order is what `keys()` and `print` show.
fn encode(val: &Value, heap: &Heap, symbols: &SymbolTable) -> Result<Json, String> {
    Ok(match val {
        Value::Nil => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(n) => json!(*n),
        Value::Float(f) => encode_float(*f),
        Value::String(id) => Json::String(heap.get_string(*id).to_string()),
        Value::List(id) => Json::Array(encode_all(heap.get_list(*id), heap, symbols)?),
        Value::F64Array(id) => {
            let values: Vec<Json> = heap.get_f64_array(*id).iter().map(|f| encode_float(*f)).collect();
            json!({ TAG: "f64_array", "values": values })
        }
        Value::Map(id) => {
            let map = heap.get_map(*id);
            let class = heap.map_class_name(*id);
            let mut plain = class.is_none();
            let mut prev: Option<&str> = None;
            let mut fields = Vec::with_capacity(map.len());
            for (k, v) in map.iter() {
                let k: &str = k;
                if k == TAG || prev.is_some_and(|p| p >= k) {
                    plain = false;
                }
                prev = Some(k);
                fields.push((k, encode(v, heap, symbols)?));
            }
            if plain {
                Json::Object(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
            } else {
                let fields: Vec<Json> = fields.into_iter().map(|(k, v)| json!([k, v])).collect();
                match class {
                    Some(class) => json!({ TAG: "instance", "class": class, "fields": fields }),
                    None => json!({ TAG: "record", "fields": fields }),
                }
            }
        }
        Value::EnumVariant { tag, data } => json!({
            TAG: "enum",
            "tag": heap.get_string(*tag),
            "data": encode_all(heap.get_list(*data), heap, symbols)?,
        }),
        Value::Dual { value, derivative } => json!({
            TAG: "dual",
            "value": encode_float(*value),
            "derivative": encode_float(*derivative),
        }),
        Value::Vec2(x, y) => json!({ TAG: "vec2", "x": encode_float(*x), "y": encode_float(*y) }),
        Value::Vec3(id) => {
            let [x, y, z] = heap.get_vec3(*id);
            json!({ TAG: "vec3", "x": encode_float(x), "y": encode_float(y), "z": encode_float(z) })
        }
        Value::Symbol(id) => match symbols.name(*id) {
            Some(name) => json!({ TAG: "symbol", "name": name }),
            None => return Err("an unnamed symbol".to_string()),
        },
        // Only a `var` binding holds a cell, and reading one yields its
        // contents, so a cell inside a value is not expected; store what a
        // read would see.
        Value::Cell(id) => encode(&heap.cell_read(*id), heap, symbols)?,
        Value::Closure(_) | Value::OverloadSet(_) | Value::NativeFunction(_) => {
            return Err("a function".to_string());
        }
        Value::Element(_) => return Err("a UI element".to_string()),
        Value::Handle(_) => return Err("a host handle".to_string()),
        Value::Pending(_) => return Err("a pending value".to_string()),
    })
}

fn encode_all(vals: &[Value], heap: &Heap, symbols: &SymbolTable) -> Result<Vec<Json>, String> {
    vals.iter().map(|v| encode(v, heap, symbols)).collect()
}

/// A float as a JSON number, or tagged when JSON has no number for it.
fn encode_float(f: f64) -> Json {
    if f.is_finite() {
        return json!(f);
    }
    let name = if f.is_nan() {
        "nan"
    } else if f > 0.0 {
        "inf"
    } else {
        "-inf"
    };
    json!({ TAG: "float", "value": name })
}

/// Rebuild a value [`encode`] wrote. An untagged JSON value is always
/// readable; a tagged one must be a shape `encode` writes.
fn decode(json: &Json, heap: &mut Heap, symbols: &mut SymbolTable) -> Result<Value, String> {
    Ok(match json {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Value::Int(i),
            None => Value::Float(n.as_f64().ok_or("a number that is out of range")?),
        },
        Json::String(s) => Value::String(heap.alloc_string(s.clone())),
        Json::Array(items) => {
            let elems = decode_all(items, heap, symbols)?;
            Value::List(heap.alloc_list(elems))
        }
        Json::Object(obj) => match obj.get(TAG) {
            None => {
                let mut entries = crate::heap::record_map_with_capacity(obj.len());
                for (k, v) in obj {
                    entries.insert(k.clone(), decode(v, heap, symbols)?);
                }
                Value::Map(heap.alloc_map(entries))
            }
            Some(tag) => decode_tagged(tag, obj, heap, symbols)?,
        },
    })
}

fn decode_all(
    items: &[Json],
    heap: &mut Heap,
    symbols: &mut SymbolTable,
) -> Result<Vec<Value>, String> {
    items.iter().map(|v| decode(v, heap, symbols)).collect()
}

fn decode_tagged(
    tag: &Json,
    obj: &serde_json::Map<String, Json>,
    heap: &mut Heap,
    symbols: &mut SymbolTable,
) -> Result<Value, String> {
    let tag = tag
        .as_str()
        .ok_or_else(|| format!("unreadable value tag {tag}"))?;
    let bad = || format!("unreadable `{tag}` value {}", Json::Object(obj.clone()));
    let float = |field: &str| obj.get(field).and_then(decode_float).ok_or_else(bad);
    let text = |field: &str| obj.get(field).and_then(Json::as_str).ok_or_else(bad);
    let list = |field: &str| obj.get(field).and_then(Json::as_array).ok_or_else(bad);
    Ok(match tag {
        "float" => Value::Float(decode_float(&Json::Object(obj.clone())).ok_or_else(bad)?),
        "f64_array" => {
            let values = list("values")?
                .iter()
                .map(|v| decode_float(v).ok_or_else(bad))
                .collect::<Result<Vec<f64>, String>>()?;
            Value::F64Array(heap.alloc_f64_array(values))
        }
        "record" | "instance" => {
            let fields = list("fields")?;
            let mut entries = crate::heap::record_map_with_capacity(fields.len());
            for field in fields {
                let (name, value) = match field.as_array().map(Vec::as_slice) {
                    Some([Json::String(name), value]) => (name, value),
                    _ => return Err(bad()),
                };
                entries.insert(name.clone(), decode(value, heap, symbols)?);
            }
            if tag == "instance" {
                let class = heap.alloc_string(text("class")?.to_string());
                Value::Map(heap.alloc_class_instance(entries, class))
            } else {
                Value::Map(heap.alloc_map(entries))
            }
        }
        "enum" => {
            let data = decode_all(list("data")?, heap, symbols)?;
            Value::EnumVariant {
                tag: heap.alloc_string(text("tag")?.to_string()),
                data: heap.alloc_list(data),
            }
        }
        "dual" => Value::Dual {
            value: float("value")?,
            derivative: float("derivative")?,
        },
        "vec2" => Value::Vec2(float("x")?, float("y")?),
        "vec3" => heap.vec3_value(float("x")?, float("y")?, float("z")?),
        "symbol" => Value::Symbol(symbols.intern(text("name")?)),
        other => return Err(format!("unknown value tag \"{other}\"")),
    })
}

/// The float [`encode_float`] wrote: a number, or the tagged non-finite form.
fn decode_float(json: &Json) -> Option<f64> {
    if let Some(f) = json.as_f64() {
        return Some(f);
    }
    let obj = json.as_object()?;
    if obj.get(TAG)?.as_str()? != "float" {
        return None;
    }
    match obj.get("value")?.as_str()? {
        "nan" => Some(f64::NAN),
        "inf" => Some(f64::INFINITY),
        "-inf" => Some(f64::NEG_INFINITY),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `source` in a fresh env, first loading `stored` if given, and
    /// return its output with what a save afterwards produces.
    fn run_with(source: &str, stored: Option<&Json>) -> (String, StateStorageSave) {
        let mut env = Env::new();
        let pid = env.load_program(source).unwrap();
        let sid = env.create_stack(pid).unwrap();
        if let Some(doc) = stored {
            env.load_state_storage(pid, sid, doc).unwrap();
        }
        env.run(sid).unwrap();
        let out = env.take_output().join("\n");
        (out, env.save_state_storage(pid, sid))
    }

    /// Run `source` twice, each in its own env, carrying the state over.
    fn two_runs(source: &str) -> (String, String) {
        let (first, saved) = run_with(source, None);
        assert!(saved.skipped.is_empty(), "skipped: {:?}", saved.skipped);
        // Through text, as the CLI does: what matters is what a file holds.
        let text = serde_json::to_string(&saved.document).unwrap();
        let doc: Json = serde_json::from_str(&text).unwrap();
        let (second, _) = run_with(source, Some(&doc));
        (first, second)
    }

    fn load_err(source: &str, doc: Json) -> String {
        let mut env = Env::new();
        let pid = env.load_program(source).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.load_state_storage(pid, sid, &doc).err().expect("load should fail")
    }

    #[test]
    fn a_counter_counts_across_envs() {
        let (first, second) = two_runs("state hits = 0\nhits += 1\nprint(hits)\n");
        assert_eq!((first.trim(), second.trim()), ("1", "2"));
    }

    #[test]
    fn a_state_var_comes_back_as_a_cell() {
        let src = "state var n = 10\nfn bump()\n  set n = get n + 1\nend\nbump()\nprint(get n)\n";
        let (first, second) = two_runs(src);
        assert_eq!((first.trim(), second.trim()), ("11", "12"));
    }

    #[test]
    fn slots_behind_calls_loops_and_keys_come_back() {
        let src = "\
fn counter()
  state count = 0
  count += 1
  count
end
print(counter(), counter())
for i in range(2) do
  state seen = 0
  seen += 10
  print(seen)
end
for id in [\"a\", \"b\"] do
  state(id) n = 0
  n += 100
  print(n)
end
";
        let (first, second) = two_runs(src);
        assert_eq!(first, "1 1\n10\n10\n100\n100");
        assert_eq!(second, "2 2\n20\n20\n200\n200");
    }

    #[test]
    fn values_round_trip_with_their_types() {
        // The first run builds the values; the second prints what was loaded
        // (the `if` keeps the first run from printing).
        let src = "\
enum Shape
  Circle(r),
  Dot,
end
class P
  x: num,
  y: num,
end
state runs = 0
state whole = 2
state frac = 2.0
state big = 1.0e308 * 10.0
state rec = {b: 1, a: [nil, true, \"s\"]}
state shape = Circle(1.5)
state inst = P(3, 4)
state v2 = vec2(1, 2)
state v3 = vec3(1, 2, 3)
state arr = f64_array(2)
runs += 1
if runs == 2 then
  print(whole, frac, big, rec, keys(rec), shape, inst.x + inst.y, v2, v3, arr)
  print(type(whole), type(frac), type(shape), type(inst), type(v2), type(v3), type(arr))
  match shape
    when Circle(r) -> print(\"circle\", r)
    when Dot -> print(\"dot\")
  end
end
";
        // What one process prints for these values with no storage involved.
        let direct = src.replace("runs += 1", "runs += 2");
        let (expected, _) = run_with(&direct, None);
        let (first, second) = two_runs(src);
        assert_eq!(first, "");
        assert_eq!(second, expected);
        assert!(second.contains("circle 1.5"), "{second}");
    }

    #[test]
    fn a_function_is_skipped_and_named_not_stored_as_text() {
        let (_, saved) = run_with("state f = fn(x) -> x\nstate n = 1\nstate fs = [f]\n", None);
        assert_eq!(saved.saved, 1);
        assert_eq!(
            saved.skipped,
            vec![
                ("f".to_string(), "a function".to_string()),
                ("fs".to_string(), "a function".to_string()),
            ]
        );
        assert_eq!(saved.document["slots"][0]["name"], "n");
    }

    #[test]
    fn a_top_level_slot_can_be_written_by_hand() {
        let doc = json!({
            "format": "petal-state", "version": 1,
            "slots": [{ "name": "hits", "value": 41 }],
        });
        let (out, _) = run_with("state hits = 0\nhits += 1\nprint(hits)\n", Some(&doc));
        assert_eq!(out.trim(), "42");
    }

    #[test]
    fn an_undeclared_slot_is_dropped_when_others_match() {
        let (_, saved) = run_with("state a = 1\nstate b = 2\n", None);
        let mut env = Env::new();
        let pid = env.load_program("state a = 0\nprint(a)\n").unwrap();
        let sid = env.create_stack(pid).unwrap();
        let load = env.load_state_storage(pid, sid, &saved.document).unwrap();
        assert_eq!((load.restored, load.dropped), (1, vec!["b".to_string()]));
        env.run(sid).unwrap();
        assert_eq!(env.take_output(), vec!["1".to_string()]);
        assert_eq!(env.save_state_storage(pid, sid).saved, 1);
    }

    #[test]
    fn a_document_from_another_script_is_an_error() {
        let (_, saved) = run_with("state a = 1\n", None);
        let err = load_err("state other = 0\n", saved.document);
        assert!(err.contains("`a`") && err.contains("different script"), "{err}");
    }

    #[test]
    fn unusable_documents_say_why() {
        let src = "state a = 1\n";
        let cases = [
            (json!([1, 2]), "not a Petal state storage file"),
            (json!({ "a": 1 }), "not a Petal state storage file"),
            (json!({ "format": "petal-state", "version": 2, "slots": [] }), "version 2"),
            (json!({ "format": "petal-state", "version": 1 }), "no \"slots\" list"),
            (
                json!({ "format": "petal-state", "version": 1, "slots": [{ "name": "a" }] }),
                "`a` has no \"value\"",
            ),
            (
                json!({ "format": "petal-state", "version": 1,
                        "slots": [{ "name": "a", "value": { "$petal": "wat" } }] }),
                "`a`: unknown value tag \"wat\"",
            ),
            (
                json!({ "format": "petal-state", "version": 1,
                        "slots": [{ "name": "a", "key": "x", "value": 1 }] }),
                "`a`: \"key\" is not an id",
            ),
        ];
        for (doc, expected) in cases {
            let err = load_err(src, doc.clone());
            assert!(err.contains(expected), "{doc} gave: {err}");
        }
    }

    #[test]
    fn a_record_field_named_like_the_tag_is_escaped() {
        let src = "state r = {}\nif len(keys(r)) == 0 then\n  r[\"$petal\"] = \"vec2\"\nend\nprint(r)\n";
        let (first, second) = two_runs(src);
        assert_eq!(first, second);
    }
}
