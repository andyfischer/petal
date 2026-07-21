//! [`Provider`] — the generic query-request / provider-response core.
//!
//! A provider is "a web server for data": it answers `query(kind, arg)` calls
//! against a per-run state `S`, and optionally reacts to fire-and-forget
//! `emit(event, arg)` signals. That's the whole abstraction — it knows nothing
//! about panes, UI scripts, or any particular transport, so it drops into any
//! query/response setting.
//!
//! ```no_run
//! use std::time::Duration;
//! use petal_query::{Provider, Reply};
//!
//! # fn git_log(repo: &str) -> serde_json::Value { serde_json::Value::Null }
//! Provider::new(|init| init.repo_arg())          // build state from the handshake
//!     .query("log", |repo: &mut String, ctx| {
//!         Reply::json(git_log(repo)).max_age(Duration::from_secs(3))
//!     });
//! ```
//!
//! To expose a provider over the Garden Pane Protocol (ship a UI script, name
//! the pane, run the stdio loop), hand it to [`crate::gpp::serve`] — the
//! GPP-specific presentation (pane name + UI script) lives there, *not* here.

use std::collections::HashMap;

use serde::Serialize;

use crate::cache_control::CachePolicy;
use crate::wire::InitializeParams;

/// A provider's answer to one `query(kind, arg)`: the value (or an error) plus
/// how cacheable it is. Build one with [`Reply::json`] / [`Reply::error`] /
/// [`Reply::loading`], then attach a policy with the builder methods (the
/// default is [`CachePolicy::forever`]).
pub struct Reply {
    /// `Some(Ok(v))` = a value; `Some(Err(e))` = an error; `None` = still
    /// loading (the host keeps the script spinning without re-requesting).
    outcome: Option<Result<serde_json::Value, String>>,
    policy: CachePolicy,
}

impl Reply {
    /// A successful answer carrying `value` (anything `Serialize`), cached
    /// [`forever`](CachePolicy::forever) by default.
    pub fn json(value: impl Serialize) -> Reply {
        Reply {
            outcome: Some(Ok(
                serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
            )),
            policy: CachePolicy::forever(),
        }
    }

    /// A successful answer from an already-built JSON value.
    pub fn value(value: serde_json::Value) -> Reply {
        Reply {
            outcome: Some(Ok(value)),
            policy: CachePolicy::forever(),
        }
    }

    /// An error answer; the script surfaces `message` via `error_of` / `??`.
    pub fn error(message: impl Into<String>) -> Reply {
        Reply {
            outcome: Some(Err(message.into())),
            policy: CachePolicy::forever(),
        }
    }

    /// "Still loading": neither value nor error. The host keeps the script's
    /// spinner up without re-requesting until the app pushes an
    /// [`invalidate`](crate::wire::method::INVALIDATE) or the key is re-queried.
    /// Use when the real work is happening on a background thread the handler
    /// only polls.
    pub fn loading() -> Reply {
        Reply {
            outcome: None,
            policy: CachePolicy::forever(),
        }
    }

    /// Convert a `Result<T, E>` into a reply (Ok → value, Err → error), cached
    /// forever by default. Lets a handler `return work().into()`.
    pub fn from_result<T: Serialize, E: ToString>(result: Result<T, E>) -> Reply {
        match result {
            Ok(v) => Reply::json(v),
            Err(e) => Reply::error(e.to_string()),
        }
    }

    /// Attach an explicit [`CachePolicy`].
    pub fn cache(mut self, policy: CachePolicy) -> Reply {
        self.policy = policy;
        self
    }

    /// Shorthand for [`cache`](Self::cache)`(CachePolicy::max_age(d))`.
    pub fn max_age(mut self, max_age: std::time::Duration) -> Reply {
        self.policy = CachePolicy::max_age(max_age);
        self
    }

    /// Shorthand for [`cache`](Self::cache)`(CachePolicy::no_store())` — live
    /// data, always revalidated.
    pub fn no_store(mut self) -> Reply {
        self.policy = CachePolicy::no_store();
        self
    }

    /// Shorthand for [`cache`](Self::cache)`(CachePolicy::forever())` (the
    /// default; explicit for readability at a call site).
    pub fn forever(mut self) -> Reply {
        self.policy = CachePolicy::forever();
        self
    }

    /// Split into `(value, error, policy)` for a transport.
    pub(crate) fn into_parts(self) -> (Option<serde_json::Value>, Option<String>, CachePolicy) {
        match self.outcome {
            Some(Ok(v)) => (Some(v), None, self.policy),
            Some(Err(e)) => (None, Some(e), self.policy),
            None => (None, None, self.policy),
        }
    }
}

impl<T: Serialize, E: ToString> From<Result<T, E>> for Reply {
    fn from(result: Result<T, E>) -> Reply {
        Reply::from_result(result)
    }
}

/// The context handed to a query handler: the requested `kind`/`arg` and the
/// handshake [`InitializeParams`] (launch args, cwd, pane size).
pub struct QueryContext<'a> {
    /// The query kind (matches the registered handler; useful when one closure
    /// serves several kinds).
    pub kind: &'a str,
    /// The query argument (a commit hash, a file path, a filter string, …).
    pub arg: &'a str,
    /// The handshake parameters, so a handler can consult `cwd`/`args`/size.
    pub init: &'a InitializeParams,
}

/// The context handed to an `emit` handler: the `event` name and its JSON `arg`,
/// plus the handshake params.
pub struct EmitContext<'a> {
    /// The emitted event name (matches the registered handler).
    pub event: &'a str,
    /// The emitted argument — any JSON tree the script passed.
    pub arg: &'a serde_json::Value,
    /// The handshake parameters.
    pub init: &'a InitializeParams,
}

type QueryHandler<S> = Box<dyn FnMut(&mut S, &QueryContext) -> Reply>;
type EmitHandler<S> = Box<dyn FnMut(&mut S, &EmitContext)>;

/// A query/response provider over a per-run state `S`: a set of `kind` → handler
/// and `event` → handler registrations, plus a `build_state` that materializes
/// `S` from the handshake. Build one with [`Provider::new`] /
/// [`Provider::stateless`], register handlers, then drive it — generically via
/// [`answer`](Self::answer) / [`handle_emit`](Self::handle_emit), or over the
/// Garden Pane Protocol via [`crate::gpp::serve`].
///
/// The provider owns *no* presentation: no pane name, no UI script, no
/// transport. Those are supplied by whatever runs it.
pub struct Provider<S> {
    pub(crate) build_state: Box<dyn FnOnce(&InitializeParams) -> S>,
    pub(crate) query_handlers: HashMap<String, QueryHandler<S>>,
    pub(crate) emit_handlers: HashMap<String, EmitHandler<S>>,
}

impl Provider<()> {
    /// A stateless provider. Handlers receive `&mut ()`.
    pub fn stateless() -> Provider<()> {
        Provider::new(|_| ())
    }
}

impl<S: 'static> Provider<S> {
    /// A new provider whose per-run state is built from the handshake
    /// [`InitializeParams`] (launch args, cwd) by `build_state`.
    pub fn new(build_state: impl FnOnce(&InitializeParams) -> S + 'static) -> Provider<S> {
        Provider {
            build_state: Box::new(build_state),
            query_handlers: HashMap::new(),
            emit_handlers: HashMap::new(),
        }
    }

    /// Register a handler for `query(kind, …)`. The handler receives the state
    /// by `&mut` reference and the [`QueryContext`], and returns a [`Reply`].
    /// Fluent: chain `.query(...)` calls.
    pub fn query(
        mut self,
        kind: impl Into<String>,
        handler: impl FnMut(&mut S, &QueryContext) -> Reply + 'static,
    ) -> Provider<S> {
        self.query_handlers.insert(kind.into(), Box::new(handler));
        self
    }

    /// Register a handler for `emit(event, …)`. Fire-and-forget: the handler
    /// acts on the signal (persist state, open a path) and returns nothing.
    pub fn on_emit(
        mut self,
        event: impl Into<String>,
        handler: impl FnMut(&mut S, &EmitContext) + 'static,
    ) -> Provider<S> {
        self.emit_handlers.insert(event.into(), Box::new(handler));
        self
    }

    /// Build the per-run state from the handshake params (consumes the
    /// `build_state` closure). Call once, before serving queries.
    pub fn build(&mut self, init: &InitializeParams) -> S {
        // Swap in a no-op so `build_state` (a FnOnce) can be moved out; it is
        // only ever called once, so the placeholder is never invoked.
        let build = std::mem::replace(&mut self.build_state, Box::new(|_| unreachable!()));
        build(init)
    }

    /// Answer one `query(kind, arg)` against `state`: dispatch to the registered
    /// handler, or [`Reply::value(null)`](Reply::value) for an unregistered
    /// kind. The generic entry point — any transport can drive it.
    pub fn answer(&mut self, state: &mut S, ctx: &QueryContext) -> Reply {
        match self.query_handlers.get_mut(ctx.kind) {
            Some(handler) => handler(state, ctx),
            None => Reply::value(serde_json::Value::Null),
        }
    }

    /// Deliver one `emit(event, arg)` to its handler, if registered (else a
    /// no-op). The generic entry point for the fire-and-forget channel.
    pub fn handle_emit(&mut self, state: &mut S, ctx: &EmitContext) {
        if let Some(handler) = self.emit_handlers.get_mut(ctx.event) {
            handler(state, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn init() -> InitializeParams {
        InitializeParams {
            pane_id: 0,
            rows: 40,
            cols: 120,
            args: vec!["/repo".to_string()],
            cwd: "/repo".to_string(),
        }
    }

    #[test]
    fn answer_dispatches_to_the_registered_handler() {
        let mut p = Provider::new(|init| init.args.first().cloned().unwrap_or_default())
            .query("log", |repo: &mut String, _ctx| Reply::json(json!({ "repo": repo.clone() })));
        let init = init();
        let mut state = p.build(&init);
        let reply = p.answer(
            &mut state,
            &QueryContext { kind: "log", arg: "", init: &init },
        );
        let (v, e, _) = reply.into_parts();
        assert_eq!(v.unwrap()["repo"], "/repo");
        assert!(e.is_none());
    }

    #[test]
    fn unregistered_kind_answers_null() {
        let mut p = Provider::stateless();
        let init = init();
        let mut state = p.build(&init);
        let reply = p.answer(
            &mut state,
            &QueryContext { kind: "nope", arg: "", init: &init },
        );
        let (v, _, _) = reply.into_parts();
        assert_eq!(v.unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn emit_reaches_its_handler_and_mutates_state() {
        let mut p = Provider::new(|_| 0i64).on_emit("ui_state", |s: &mut i64, ctx| {
            *s = ctx.arg["left_frac"].as_i64().unwrap_or(0);
        });
        let init = init();
        let mut state = p.build(&init);
        let arg = json!({ "left_frac": 300 });
        p.handle_emit(
            &mut state,
            &EmitContext { event: "ui_state", arg: &arg, init: &init },
        );
        assert_eq!(state, 300);
    }

    #[test]
    fn from_result_maps_ok_and_err() {
        let ok: Reply = Ok::<_, String>(json!({ "x": 1 })).into();
        let (v, e, _) = ok.into_parts();
        assert_eq!(v.unwrap()["x"], 1);
        assert!(e.is_none());

        let err: Reply = Err::<serde_json::Value, _>("boom".to_string()).into();
        let (v, e, _) = err.into_parts();
        assert!(v.is_none());
        assert_eq!(e.unwrap(), "boom");
    }
}
