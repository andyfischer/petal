//! `panel_store_get(key)` / `panel_store_set(key, value)` — a panel's own
//! persistent key/value store.
//!
//! A panel script's `state` lives and dies with the process: close the window
//! and a todo list, a note, a set of preferences is gone. There is deliberately
//! no file API in the panel vocabulary (a sketch that can open any path is a
//! different security story), so this is the narrow, boring alternative: a
//! string→string map, **scoped to the script's own path**, that survives a
//! restart. Two panels running different scripts cannot see each other's keys,
//! and a panel cannot name a file.
//!
//! ```petal
//! state todos = json_parse(panel_store_get("todos") ?? "[]")
//! # …after editing…
//! panel_store_set("todos", json_stringify(todos))
//! ```
//!
//! Values are strings because that is the format a script can already produce
//! for anything (`str`, JSON) and the only one whose on-disk meaning is
//! unambiguous. `panel_store_set(key, nil)` deletes a key.
//!
//! ## Where it lives
//!
//! One JSON file per script under `~/.garden/panel-store/`, named after the
//! script's absolute path (slugified, plus a hash so two same-named scripts in
//! different directories don't collide). `GARDEN_PANEL_STORE_DIR` overrides the
//! directory — how tests get a scratch store, and the escape hatch for a
//! sandboxed run.
//!
//! ## What it is not
//!
//! Not a database: the whole map is read once and rewritten (atomically, via a
//! temp file and a rename) whenever a frame changed it. That is right for the
//! kilobyte of state a panel keeps and wrong for anything bigger, which is why
//! both the value size and the entry count are capped — a runaway script gets
//! an error, not a full disk.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use petal::env::Env;
use petal::native_fn::{NativeClass, NativeResult, PetalCxt};
use petal::value::Value;

/// Largest value a script may store, in bytes. Generous for the settings and
/// small JSON documents this is for; small enough that a loop appending to a
/// key fails loudly instead of growing a file without bound.
const MAX_VALUE_BYTES: usize = 256 * 1024;

/// Largest number of keys one script's store may hold.
const MAX_ENTRIES: usize = 1024;

thread_local! {
    /// The store the panel currently running a frame on this thread may reach,
    /// installed by the host for the duration of the run (the same swap dance
    /// the data/query providers use). `None` — a host that never made one —
    /// means every read answers nil and every write errors, rather than a
    /// script silently writing into someone else's file.
    static ACTIVE: RefCell<Option<PanelStore>> = const { RefCell::new(None) };
}

/// One panel's persisted map: the file it lives in, its contents, and whether
/// this frame changed them.
#[derive(Debug)]
pub struct PanelStore {
    file: PathBuf,
    entries: BTreeMap<String, String>,
    dirty: bool,
}

impl PanelStore {
    /// Open (or start) the store belonging to `script`. A missing or unreadable
    /// file is an empty store — a first run and a corrupt file both leave the
    /// panel working, which matters more here than reporting a problem the user
    /// cannot act on. A store whose directory can't be resolved (no `$HOME`)
    /// still works in memory for the session; only the flush fails.
    pub fn for_script(script: &Path) -> PanelStore {
        let file = store_file(script);
        let entries = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|text| serde_json::from_str::<BTreeMap<String, String>>(&text).ok())
            .unwrap_or_default();
        PanelStore {
            file: file.unwrap_or_default(),
            entries,
            dirty: false,
        }
    }

    /// An in-memory store backed by an explicit file — the unit-test seam.
    pub fn at_path(file: PathBuf) -> PanelStore {
        let entries = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str::<BTreeMap<String, String>>(&text).ok())
            .unwrap_or_default();
        PanelStore {
            file,
            entries,
            dirty: false,
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    /// Set (or, with `None`, delete) a key. Errors rather than truncating when
    /// a value or the map is over the cap, so the script learns about it.
    pub fn set(&mut self, key: &str, value: Option<String>) -> Result<(), String> {
        match value {
            None => {
                if self.entries.remove(key).is_some() {
                    self.dirty = true;
                }
            }
            Some(v) => {
                if v.len() > MAX_VALUE_BYTES {
                    return Err(format!(
                        "panel_store_set(): value for '{key}' is {} bytes, over the {MAX_VALUE_BYTES}-byte limit",
                        v.len()
                    ));
                }
                if self.entries.len() >= MAX_ENTRIES && !self.entries.contains_key(key) {
                    return Err(format!(
                        "panel_store_set(): store is full ({MAX_ENTRIES} keys)"
                    ));
                }
                if self.entries.get(key).map(String::as_str) != Some(v.as_str()) {
                    self.entries.insert(key.to_string(), v);
                    self.dirty = true;
                }
            }
        }
        Ok(())
    }

    /// Write the map back if this frame changed it. Atomic (temp file +
    /// rename), so a crash mid-write leaves the previous contents intact rather
    /// than a truncated file that would read back as an empty store.
    pub fn flush(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        if self.file.as_os_str().is_empty() {
            return Err("panel store has no file (is $HOME set?)".to_string());
        }
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("panel store: cannot create {}: {e}", dir.display()))?;
        }
        let json = serde_json::to_string_pretty(&self.entries)
            .map_err(|e| format!("panel store: cannot encode: {e}"))?;
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, json)
            .map_err(|e| format!("panel store: cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.file)
            .map_err(|e| format!("panel store: cannot replace {}: {e}", self.file.display()))?;
        self.dirty = false;
        Ok(())
    }

    /// Whether an un-flushed change is pending.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
}

/// The directory panel stores live in: `$GARDEN_PANEL_STORE_DIR`, else
/// `~/.garden/panel-store`. `None` when neither resolves.
fn store_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("GARDEN_PANEL_STORE_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".garden").join("panel-store"))
}

/// The file one script's store lives in. The name is readable (so a user can
/// find and delete it) *and* collision-free: a slug of the path's last two
/// components, plus a hash of the whole absolute path.
fn store_file(script: &Path) -> Option<PathBuf> {
    let abs = std::fs::canonicalize(script)
        .or_else(|_| std::env::current_dir().map(|cwd| cwd.join(script)))
        .unwrap_or_else(|_| script.to_path_buf());
    let key = abs.to_string_lossy();
    let mut slug: String = key
        .chars()
        .rev()
        .take(48)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        slug.push_str("panel");
    }
    Some(store_dir()?.join(format!("{slug}-{:016x}.json", fnv1a(key.as_bytes()))))
}

/// FNV-1a, 64-bit. Spelled out rather than reaching for `DefaultHasher`
/// because this hash names a file that must still resolve after a toolchain
/// upgrade — `DefaultHasher`'s output is explicitly not stable across releases.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// Install `store` as this thread's active panel store for the duration of a
/// frame, returning the previous one so the host can swap it back (panic-safe,
/// mirroring the data/query providers).
pub(crate) fn swap_store(store: Option<PanelStore>) -> Option<PanelStore> {
    ACTIVE.with(|s| std::mem::replace(&mut *s.borrow_mut(), store))
}

/// Register the two store natives.
pub(crate) fn register_store(env: &mut Env) {
    env.register_native("panel_store_get", native_store_get);
    let set = env.register_native("panel_store_set", native_store_set);
    // A write is an effect: with a `Pending` argument the call must be a no-op
    // rather than being absorbed as its result (a half-loaded value must never
    // be what gets persisted).
    env.set_native_class(set, NativeClass::Effectful);
}

/// `panel_store_get(key)` → the stored string, or nil if this panel never
/// stored that key.
fn native_store_get(cxt: &mut PetalCxt) -> NativeResult {
    let key = cxt.get_string(1)?;
    let found = ACTIVE.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|store| store.get(&key).map(str::to_string))
    });
    match found {
        Some(v) => {
            let id = cxt.heap_mut().alloc_string(v);
            cxt.push_value(Value::String(id));
        }
        None => cxt.push_nil(),
    }
    Ok(1)
}

/// `panel_store_set(key, value)` — persist a string under `key` for this
/// script. `nil` deletes the key. Returns nil.
fn native_store_set(cxt: &mut PetalCxt) -> NativeResult {
    let key = cxt.get_string(1)?;
    let value = match cxt.get_value(2)? {
        Value::Nil => None,
        Value::String(id) => Some(cxt.heap().get_string(id).to_string()),
        other => {
            return Err(format!(
                "panel_store_set() value must be a string or nil, got {} — encode it first (e.g. json_stringify)",
                other.type_name()
            ));
        }
    };
    let result = ACTIVE.with(|s| match s.borrow_mut().as_mut() {
        Some(store) => store.set(&key, value),
        None => Err("panel_store_set(): this panel has no store".to_string()),
    });
    result?;
    cxt.push_nil();
    Ok(1)
}

/// Serializes the tests that point `GARDEN_PANEL_STORE_DIR` at a scratch
/// directory. `set_var`/`remove_var` are process-global, and the test binary
/// runs tests in parallel — without this, one test's teardown silently sends
/// another test's store to the real `~/.garden`.
#[cfg(test)]
pub(crate) static STORE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take [`STORE_ENV_LOCK`], ignoring poisoning (a panicking test has already
/// reported its own failure; blocking every later one behind it has not).
#[cfg(test)]
pub(crate) fn lock_store_env() -> std::sync::MutexGuard<'static, ()> {
    STORE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_and_delete_round_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("store.json");

        let mut store = PanelStore::at_path(file.clone());
        assert_eq!(store.get("todos"), None);
        store.set("todos", Some("[1,2]".into())).unwrap();
        assert!(store.is_dirty());
        store.flush().unwrap();
        assert!(!store.is_dirty(), "flush clears the dirty flag");

        // A fresh store over the same file sees the persisted value.
        let mut reopened = PanelStore::at_path(file.clone());
        assert_eq!(reopened.get("todos"), Some("[1,2]"));
        reopened.set("todos", None).unwrap();
        reopened.flush().unwrap();
        assert_eq!(PanelStore::at_path(file).get("todos"), None);
    }

    #[test]
    fn an_unchanged_frame_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("store.json");
        let mut store = PanelStore::at_path(file.clone());
        store.set("k", Some("v".into())).unwrap();
        store.flush().unwrap();
        // Setting the same value again is not a change, so no rewrite is due.
        store.set("k", Some("v".into())).unwrap();
        assert!(!store.is_dirty());
    }

    #[test]
    fn oversized_values_and_full_stores_are_refused() {
        let mut store = PanelStore::at_path(PathBuf::from("/nonexistent/store.json"));
        let huge = "x".repeat(MAX_VALUE_BYTES + 1);
        assert!(store.set("k", Some(huge)).unwrap_err().contains("limit"));
        for i in 0..MAX_ENTRIES {
            store.set(&format!("k{i}"), Some("v".into())).unwrap();
        }
        assert!(store
            .set("one-more", Some("v".into()))
            .unwrap_err()
            .contains("full"));
        // …but overwriting an existing key still works when full.
        store.set("k0", Some("w".into())).unwrap();
    }

    #[test]
    fn corrupt_store_files_read_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("store.json");
        std::fs::write(&file, "{not json").unwrap();
        assert_eq!(PanelStore::at_path(file).get("anything"), None);
    }

    #[test]
    fn two_scripts_get_two_files() {
        let _guard = lock_store_env();
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: guarded by STORE_ENV_LOCK, so no other test reads the var
        // while this one owns it.
        unsafe { std::env::set_var("GARDEN_PANEL_STORE_DIR", dir.path()) };
        let a = store_file(Path::new("/tmp/app-one/panel.ptl")).unwrap();
        let b = store_file(Path::new("/tmp/app-two/panel.ptl")).unwrap();
        assert_ne!(a, b, "same basename in different dirs must not collide");
        assert_eq!(a.parent().unwrap(), dir.path());
        // Stable across calls, or a restart would lose the file.
        assert_eq!(a, store_file(Path::new("/tmp/app-one/panel.ptl")).unwrap());
        unsafe { std::env::remove_var("GARDEN_PANEL_STORE_DIR") };
    }
}
