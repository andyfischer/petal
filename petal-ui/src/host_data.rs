//! The host→script data-pull channel: the `host_data(kind, arg)` native.
//!
//! petal-ui standardizes the input stream (Layer 0) and the draw buffer
//! (Layer 2, including `DrawCommand::Host` for script→host extension), but a
//! script pulling data *from* its embedder on demand had no blessed contract —
//! every host invented its own. This module blesses one, so a script that
//! fetches host data is portable across embedders.
//!
//! A script calls `host_data(kind, arg)`; the host answers with a plain
//! [`HostData`] value tree (`Nil`/`Bool`/`Int`/`Str`/`List`/`Record`), which
//! the native converts into an ordinary Petal value. The host attaches a
//! [`DataProvider`] — `Box<dyn FnMut(&str, &str) -> HostData>` — for the
//! duration of each `env.run`; without one the native answers nil, so a script
//! degrades gracefully when a host offers no data source.
//!
//! The provider runs **synchronously inside the frame**, so implementations
//! should answer from a cache or a quick lookup. This lets a script bake only
//! an index (e.g. a commit list) and lazily fetch one item's detail (a commit's
//! diff) on selection, instead of shipping megabytes up front.
//!
//! ## Host wiring
//!
//! Natives are plain fn pointers, so the provider reaches the native through a
//! thread-local side channel (the same pattern the input/frame uniforms use).
//! A host owns its provider between frames and swaps it into the channel around
//! the run:
//!
//! ```no_run
//! # use petal_ui::host_data::{swap_data_provider, DataProvider};
//! # fn run(provider: &mut Option<DataProvider>, body: impl FnOnce()) {
//! let saved = swap_data_provider(provider.take());
//! body(); // env.run(...) — the script's host_data() calls hit the provider
//! *provider = swap_data_provider(saved);
//! # }
//! ```
//!
//! [`crate::harness::Headless::set_data_provider`] does exactly this, so widget
//! logic that pulls host data is unit-testable with no renderer attached.

use std::cell::RefCell;

use indexmap::IndexMap;
use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::value::Value;

/// Plain-data value tree a host's data provider returns — the host→script
/// counterpart of a draw command (no Petal heap types leak across the crate
/// boundary; the conversion to a `Value` happens inside the native). A
/// [`Record`](HostData::Record) becomes a Petal record (`Value::Map`) with its
/// field order preserved.
#[derive(Clone, Debug, PartialEq)]
pub enum HostData {
    Nil,
    Bool(bool),
    Int(i64),
    Str(String),
    List(Vec<HostData>),
    Record(Vec<(String, HostData)>),
}

/// A host-side, on-demand data source behind the `host_data(kind, arg)` native.
/// Called synchronously inside the frame, so implementations should answer from
/// a cache or a quick lookup rather than blocking.
pub type DataProvider = Box<dyn FnMut(&str, &str) -> HostData>;

thread_local! {
    /// The provider of the run currently in progress on this thread. A host
    /// installs its provider here for the duration of `env.run` via
    /// [`swap_data_provider`]; the native reads it. `None` → nil answers.
    static DATA_PROVIDER: RefCell<Option<DataProvider>> = const { RefCell::new(None) };
}

/// Install `provider` as this thread's active data provider, returning whatever
/// was installed before. Call it with the host's provider before `env.run`, and
/// again with the saved value afterwards to restore the channel and take the
/// provider (with its updated cache) back:
///
/// ```no_run
/// # use petal_ui::host_data::{swap_data_provider, DataProvider};
/// # let mut owned: Option<DataProvider> = None;
/// let saved = swap_data_provider(owned.take());
/// // env.run(...) here
/// owned = swap_data_provider(saved);
/// ```
///
/// The swap makes nesting well-behaved; a panic in the run leaves the provider
/// in the channel, so hosts that catch panics should clear it with
/// `swap_data_provider(None)`.
pub fn swap_data_provider(provider: Option<DataProvider>) -> Option<DataProvider> {
    DATA_PROVIDER.with(|p| std::mem::replace(&mut *p.borrow_mut(), provider))
}

/// Register the standard `host_data(kind, arg)` native. Included in
/// [`crate::register_all`]; a host that composes registration itself can call
/// this directly. Safe to register even with no provider attached — the native
/// simply answers nil.
pub fn register_host_data(env: &mut Env) {
    env.register_native("host_data", native_host_data);
}

/// `host_data(kind, arg)` — ask the host's attached [`DataProvider`] for a
/// value. Without a provider it answers nil, so a script that pulls host data
/// still runs (degraded) under a host that attaches none.
fn native_host_data(cxt: &mut PetalCxt) -> NativeResult {
    let kind = cxt.get_string(1)?;
    let arg = cxt.get_string(2)?;
    let data = DATA_PROVIDER.with(|p| {
        p.borrow_mut()
            .as_mut()
            .map(|provider| provider(&kind, &arg))
    });
    let value = data_to_value(cxt, &data.unwrap_or(HostData::Nil));
    cxt.push_value(value);
    Ok(1)
}

/// Convert a [`HostData`] tree into a heap-allocated Petal [`Value`].
fn data_to_value(cxt: &mut PetalCxt, data: &HostData) -> Value {
    match data {
        HostData::Nil => Value::Nil,
        HostData::Bool(b) => Value::Bool(*b),
        HostData::Int(n) => Value::Int(*n),
        HostData::Str(s) => Value::String(cxt.heap_mut().alloc_string(s.clone())),
        HostData::List(items) => {
            let values: Vec<Value> = items.iter().map(|d| data_to_value(cxt, d)).collect();
            Value::List(cxt.heap_mut().alloc_list(values))
        }
        HostData::Record(fields) => {
            let mut map = IndexMap::new();
            for (name, d) in fields {
                let v = data_to_value(cxt, d);
                map.insert(name.clone(), v);
            }
            Value::Map(cxt.heap_mut().alloc_map(map))
        }
    }
}
