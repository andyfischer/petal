//! [`App`] — the elegant native API for a panel-mode query provider.
//!
//! A GPP panel-mode app is "a web server for one page": it ships a Petal UI
//! script once (the page), then answers the `query(kind, arg)` calls that script
//! makes (the data). Before `petal-query`, every app hand-rolled the same stdio
//! handshake, the same `match kind` dispatch, and the same `QueryResult`
//! plumbing. [`App`] distills that to a declaration:
//!
//! ```no_run
//! use std::time::Duration;
//! use petal_query::{App, CachePolicy, Reply};
//!
//! const UI: &str = "/* … a Petal drawer … */";
//!
//! # fn git_log(repo: &str) -> serde_json::Value { serde_json::Value::Null }
//! # fn git_commit(repo: &str, hash: &str) -> serde_json::Value { serde_json::Value::Null }
//! App::new("git-log", UI, |init| init.repo_arg())   // build state from the handshake
//!     .query("log", |repo, ctx| {
//!         Reply::json(git_log(repo)).max_age(Duration::from_secs(3))
//!     })
//!     .query("commit", |repo, ctx| {
//!         // A commit addressed by hash never changes — cache it forever.
//!         Reply::json(git_commit(repo, ctx.arg))
//!     })
//!     .serve()
//!     .expect("petal-query app");
//! ```
//!
//! - **State** (`S`) is built from [`InitializeParams`] at the handshake and
//!   handed to every handler by `&mut` reference, so a provider can keep caches,
//!   a repo path, a parsed transcript — whatever it needs. Stateless apps use
//!   [`App::stateless`].
//! - **Handlers** map a `kind` to a [`Reply`] carrying the value (or an error)
//!   and its [`CachePolicy`]. Unregistered kinds answer `null`.
//! - **`emit` handlers** (optional) receive the script's `emit(event, arg)`
//!   signals — the channel for persisting UI state, opening files, etc.
//! - [`serve`](App::serve) runs the whole protocol loop until `shutdown`/EOF.
//!
//! The app *never draws and never handles keys* — the host runs the script for
//! that. The app only ships the script and answers data requests, so its logic
//! stays plain Rust (shell out to `git`, read files, hit a network service).

use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use serde::Serialize;

use crate::cache_control::CachePolicy;
use crate::wire::{
    self, Envelope, EmitParams, InitializeParams, InitializeResult, QueryParams, QueryResult,
    SetScriptParams, method,
};

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

    /// Split into `(value, error, policy)` for the wire.
    fn into_parts(self) -> (Option<serde_json::Value>, Option<String>, CachePolicy) {
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

/// A panel-mode query provider: a Petal UI script plus a set of `kind` → handler
/// and `event` → handler registrations, over a per-app state `S`. Build one with
/// [`App::new`] / [`App::stateless`], register handlers, then [`serve`](App::serve).
pub struct App<S> {
    name: String,
    script: String,
    build_state: Box<dyn FnOnce(&InitializeParams) -> S>,
    title_fn: Option<Box<dyn FnOnce(&S) -> String>>,
    query_handlers: HashMap<String, QueryHandler<S>>,
    emit_handlers: HashMap<String, EmitHandler<S>>,
}

impl App<()> {
    /// A stateless app. Handlers receive `&mut ()`.
    pub fn stateless(name: impl Into<String>, script: impl Into<String>) -> App<()> {
        App::new(name, script, |_| ())
    }
}

impl<S: 'static> App<S> {
    /// A new app whose per-run state is built from the handshake
    /// [`InitializeParams`] (launch args, cwd) by `build_state`.
    pub fn new(
        name: impl Into<String>,
        script: impl Into<String>,
        build_state: impl FnOnce(&InitializeParams) -> S + 'static,
    ) -> App<S> {
        App {
            name: name.into(),
            script: script.into(),
            build_state: Box::new(build_state),
            title_fn: None,
            query_handlers: HashMap::new(),
            emit_handlers: HashMap::new(),
        }
    }

    /// Derive the pane's display name from the built state instead of the static
    /// name passed to [`new`](Self::new). Called once, after the state is built
    /// from the handshake — so a provider can title the pane from what it just
    /// loaded (e.g. `retro — <session id>`). The name is what the host shows
    /// until the script's first render.
    pub fn title(mut self, title: impl FnOnce(&S) -> String + 'static) -> App<S> {
        self.title_fn = Some(Box::new(title));
        self
    }

    /// Register a handler for `query(kind, …)`. The handler receives the app
    /// state by `&mut` reference and the [`QueryContext`], and returns a
    /// [`Reply`]. Fluent: chain `.query(...)` calls.
    pub fn query(
        mut self,
        kind: impl Into<String>,
        handler: impl FnMut(&mut S, &QueryContext) -> Reply + 'static,
    ) -> App<S> {
        self.query_handlers.insert(kind.into(), Box::new(handler));
        self
    }

    /// Register a handler for `emit(event, …)`. Fire-and-forget: the handler
    /// acts on the signal (persist state, open a path) and returns nothing.
    pub fn on_emit(
        mut self,
        event: impl Into<String>,
        handler: impl FnMut(&mut S, &EmitContext) + 'static,
    ) -> App<S> {
        self.emit_handlers.insert(event.into(), Box::new(handler));
        self
    }

    /// Run the panel-mode protocol loop on stdio until `shutdown` or EOF:
    /// handshake in panel mode, push the script, then dispatch every `query` to
    /// its handler (answering with the value/error and cache policy) and every
    /// `emit` to its handler. Blocks the calling thread; this is an app's
    /// `main`.
    pub fn serve(self) -> io::Result<()> {
        let stdin = io::stdin();
        let mut reader = io::BufReader::new(stdin.lock());
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        self.serve_on(&mut reader, &mut writer)
    }

    /// [`serve`](Self::serve) over explicit streams — the seam the tests drive.
    pub fn serve_on<R: BufRead, W: Write>(
        mut self,
        reader: &mut R,
        writer: &mut W,
    ) -> io::Result<()> {
        // 1. Handshake: read `initialize`, build state, reply in panel mode.
        let init_env = match wire::read_message(reader)? {
            Some(env) if env.is_method(method::INITIALIZE) => env,
            _ => return Ok(()), // EOF or unexpected first message
        };
        let id = init_env.id.unwrap_or(1);
        let init: InitializeParams = init_env
            .params_as()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let build_state = self.build_state;
        let mut state = build_state(&init);

        // A `title` closure (if any) names the pane from the just-built state;
        // otherwise the static name stands.
        let name = match self.title_fn.take() {
            Some(title) => title(&state),
            None => self.name.clone(),
        };

        wire::write_message(
            writer,
            &Envelope::response(
                id,
                InitializeResult {
                    name,
                    mode: "panel".to_string(),
                },
            ),
        )?;

        // 2. Push the UI script; the host compiles it into a panel.
        wire::write_message(
            writer,
            &Envelope::notification(
                method::SET_SCRIPT,
                SetScriptParams {
                    source: self.script.clone(),
                },
            ),
        )?;

        // 3. Answer requests until shutdown / EOF.
        while let Some(env) = wire::read_message(reader)? {
            if env.is_method(method::QUERY) {
                let req_id = env.id.unwrap_or(0);
                let q: QueryParams = match env.params_as() {
                    Ok(q) => q,
                    Err(e) => {
                        eprintln!("{}: bad query params: {e}", self.name);
                        continue;
                    }
                };
                let reply = match self.query_handlers.get_mut(&q.kind) {
                    Some(handler) => handler(
                        &mut state,
                        &QueryContext {
                            kind: &q.kind,
                            arg: &q.arg,
                            init: &init,
                        },
                    ),
                    // Unregistered kind: answer null (matches the apps' `_ =>`).
                    None => Reply::value(serde_json::Value::Null),
                };
                let (value, error, policy) = reply.into_parts();
                let result = QueryResult {
                    kind: q.kind,
                    arg: q.arg,
                    value,
                    error,
                    // Omit a forever policy so the wire is unchanged for the
                    // default case.
                    cache_control: (policy != CachePolicy::forever()).then_some(policy),
                };
                wire::write_message(writer, &Envelope::response(req_id, result))?;
            } else if env.is_method(method::EMIT) {
                let p: EmitParams = match env.params_as() {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("{}: bad emit params: {e}", self.name);
                        continue;
                    }
                };
                if let Some(handler) = self.emit_handlers.get_mut(&p.event) {
                    handler(
                        &mut state,
                        &EmitContext {
                            event: &p.event,
                            arg: &p.arg,
                            init: &init,
                        },
                    );
                }
            } else if env.is_method(method::SHUTDOWN) {
                return Ok(());
            }
            // `resize`, `invalidate`, and unknown notifications need no action.
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{QueryParams, RpcError};
    use serde_json::json;
    use std::io::Cursor;

    /// Frame a sequence of client-bound messages into a stdin buffer.
    fn input(envs: Vec<Envelope>) -> Cursor<Vec<u8>> {
        let mut buf = Vec::new();
        for env in &envs {
            wire::write_message(&mut buf, env).unwrap();
        }
        Cursor::new(buf)
    }

    fn init_req() -> Envelope {
        Envelope {
            jsonrpc: "2.0".into(),
            id: Some(1),
            method: Some(method::INITIALIZE.into()),
            params: Some(json!({
                "paneId": 0, "rows": 40, "cols": 120,
                "args": ["/repo"], "cwd": "/repo"
            })),
            result: None,
            error: None,
        }
    }

    fn query_req(id: u64, kind: &str, arg: &str) -> Envelope {
        Envelope {
            jsonrpc: "2.0".into(),
            id: Some(id),
            method: Some(method::QUERY.into()),
            params: Some(serde_json::to_value(QueryParams { kind: kind.into(), arg: arg.into() }).unwrap()),
            result: None,
            error: None,
        }
    }

    fn shutdown() -> Envelope {
        Envelope::notification(method::SHUTDOWN, json!({}))
    }

    /// Parse the newline-framed responses/notifications an app wrote to stdout.
    fn output(buf: &[u8]) -> Vec<Envelope> {
        let mut reader = std::io::BufReader::new(buf);
        let mut out = Vec::new();
        while let Some(env) = wire::read_message(&mut reader).unwrap() {
            out.push(env);
        }
        out
    }

    #[test]
    fn handshake_pushes_panel_mode_and_script() {
        let mut r = input(vec![init_req(), shutdown()]);
        let mut w: Vec<u8> = Vec::new();
        App::stateless("demo", "SCRIPT")
            .serve_on(&mut r, &mut w)
            .unwrap();
        let msgs = output(&w);
        // First: initialize response (panel mode). Second: setScript.
        assert_eq!(msgs[0].result.as_ref().unwrap()["mode"], "panel");
        assert_eq!(msgs[0].result.as_ref().unwrap()["name"], "demo");
        assert!(msgs[1].is_method(method::SET_SCRIPT));
        assert_eq!(msgs[1].params.as_ref().unwrap()["source"], "SCRIPT");
    }

    #[test]
    fn dispatches_query_to_handler_with_state_and_policy() {
        let mut r = input(vec![
            init_req(),
            query_req(5, "log", ""),
            query_req(6, "commit", "abc"),
            shutdown(),
        ]);
        let mut w: Vec<u8> = Vec::new();
        // State = the repo path from args.
        App::new("git", "S", |init| init.args.first().cloned().unwrap_or_default())
            .query("log", |repo: &mut String, _ctx| {
                Reply::json(json!({ "repo": repo.clone() }))
                    .max_age(std::time::Duration::from_secs(3))
            })
            .query("commit", |_repo, ctx| {
                Reply::json(json!({ "hash": ctx.arg })) // forever by default
            })
            .serve_on(&mut r, &mut w)
            .unwrap();

        let msgs = output(&w);
        // msgs[0]=init, msgs[1]=setScript, msgs[2]=log answer, msgs[3]=commit answer
        let log = msgs[2].result.as_ref().unwrap();
        assert_eq!(log["kind"], "log");
        assert_eq!(log["value"]["repo"], "/repo");
        assert_eq!(log["cacheControl"]["maxAgeMs"], 3000);

        let commit = msgs[3].result.as_ref().unwrap();
        assert_eq!(commit["value"]["hash"], "abc");
        // A forever policy is omitted from the wire.
        assert!(commit.get("cacheControl").is_none());
    }

    #[test]
    fn unregistered_kind_answers_null() {
        let mut r = input(vec![init_req(), query_req(2, "nope", ""), shutdown()]);
        let mut w: Vec<u8> = Vec::new();
        App::stateless("demo", "S").serve_on(&mut r, &mut w).unwrap();
        let msgs = output(&w);
        assert_eq!(msgs[2].result.as_ref().unwrap()["value"], serde_json::Value::Null);
    }

    #[test]
    fn error_reply_sets_error_not_value() {
        let mut r = input(vec![init_req(), query_req(2, "log", ""), shutdown()]);
        let mut w: Vec<u8> = Vec::new();
        App::stateless("demo", "S")
            .query("log", |_s, _ctx| Reply::error("no repo"))
            .serve_on(&mut r, &mut w)
            .unwrap();
        let res = output(&w)[2].result.clone().unwrap();
        assert_eq!(res["error"], "no repo");
        assert!(res.get("value").is_none());
    }

    #[test]
    fn emit_reaches_its_handler_and_mutates_state() {
        // State holds the last persisted value; an emit updates it.
        let emit = Envelope::notification(
            method::EMIT,
            EmitParams {
                event: "ui_state".into(),
                arg: json!({ "left_frac": 300 }),
            },
        );
        // A query after the emit reads the mutated state back out.
        let mut r = input(vec![init_req(), emit, query_req(2, "state", ""), shutdown()]);
        let mut w: Vec<u8> = Vec::new();
        App::new("demo", "S", |_| 0i64)
            .on_emit("ui_state", |s: &mut i64, ctx| {
                *s = ctx.arg["left_frac"].as_i64().unwrap_or(0);
            })
            .query("state", |s: &mut i64, _ctx| Reply::json(*s))
            .serve_on(&mut r, &mut w)
            .unwrap();
        assert_eq!(output(&w)[2].result.as_ref().unwrap()["value"], 300);
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

    // Keep RpcError referenced so an unused-import lint doesn't fire if the
    // error-path tests change.
    #[allow(dead_code)]
    fn _uses_rpc_error() -> RpcError {
        RpcError { code: 0, message: String::new() }
    }
}
