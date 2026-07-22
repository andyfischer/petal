//! Garden Pane Protocol adapter: run a generic [`Provider`] as a panel-mode GPP
//! app over stdio.
//!
//! This is where the **editor/presentation** concerns live — the pane name and
//! the UI script — kept out of the transport-agnostic [`Provider`]. A GPP
//! panel-mode app is "a web server for one page": it names the pane, ships a
//! Petal UI script once (the page), then answers the `query(kind, arg)` calls
//! that script makes (the data).
//!
//! ```no_run
//! use petal_query::{Provider, Reply};
//! use petal_query::gpp::{self, PanelUi};
//!
//! const UI: &str = "/* … a Petal drawer … */";
//!
//! # fn git_log(repo: &str) -> serde_json::Value { serde_json::Value::Null }
//! let provider = Provider::new(|init| init.repo_arg())
//!     .query("log", |repo: &mut String, _ctx| Reply::json(git_log(repo)));
//! gpp::serve(provider, PanelUi::new("git-log", UI))
//!     .expect("petal-query gpp app");
//! ```
//!
//! The pane name may also be derived from the built state (a provider that
//! titles the pane from what it just loaded) via [`PanelUi::title`].

use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use crate::provider::{MutateContext, Provider, Reply};
use crate::wire::{
    self, Envelope, EmitParams, InitializeParams, InitializeResult, MutateParams, MutateResult,
    QueryParams, QueryResult, SetScriptParams, method,
};
use crate::CachePolicy;

/// The built-in mutation name for browser-style navigation between a panel's
/// screens. When a subprocess panel app declares [`screens`](PanelUi::screen) and
/// does not register its own `navigate` handler, the GPP layer answers a
/// `navigate` mutation by returning the requested screen's UI source, letting the
/// host swap screens and own the history stack. Its `arg` is `{ "screen": name }`
/// and it returns `{ "screen": name, "source": <ptl source> }`.
pub const NAVIGATE: &str = "navigate";

/// The GPP presentation for a panel-mode app: the pane name (static, or derived
/// from the built state) and the UI script the host runs. Supplied by the app —
/// the GPP layer — not by the transport-agnostic [`Provider`].
pub struct PanelUi<S> {
    name: String,
    title_fn: Option<Box<dyn FnOnce(&S) -> String>>,
    script: String,
    /// Additional navigable screens (name → UI source) beyond the home `script`.
    /// The declared set doubles as the navigation allowlist: a `navigate` mutation
    /// to an undeclared screen is refused. Empty means a single-screen app.
    screens: HashMap<String, String>,
}

impl<S> PanelUi<S> {
    /// A panel named `name`, running the home UI `script`.
    pub fn new(name: impl Into<String>, script: impl Into<String>) -> PanelUi<S> {
        PanelUi {
            name: name.into(),
            title_fn: None,
            script: script.into(),
            screens: HashMap::new(),
        }
    }

    /// Derive the pane's display name from the built state instead of the static
    /// name, called once after the state is built from the handshake — so an app
    /// can title the pane from what it just loaded (e.g. `retro — <session id>`).
    pub fn title(mut self, title: impl FnOnce(&S) -> String + 'static) -> PanelUi<S> {
        self.title_fn = Some(Box::new(title));
        self
    }

    /// Declare a navigable screen `name` served from `source`. The host's
    /// browser-style navigation (`navigate(name)`) fetches this source via the
    /// built-in [`NAVIGATE`] mutation; the set of declared screens is the
    /// navigation allowlist. Fluent — chain one `.screen(...)` per screen.
    ///
    /// The home `script` from [`new`](Self::new) is screen 0 and need not be
    /// re-declared here (the host already holds it); declare the screens it can
    /// navigate *to*. An app that wants navigation effects (logging, priming data)
    /// can instead register its own `navigate` handler with
    /// [`Provider::on_mutation`], which takes precedence over this built-in.
    pub fn screen(mut self, name: impl Into<String>, source: impl Into<String>) -> PanelUi<S> {
        self.screens.insert(name.into(), source.into());
        self
    }
}

/// Run `provider` as a panel-mode GPP app on stdio until `shutdown` / EOF,
/// presenting it with `ui` (pane name + UI script). Blocks the calling thread;
/// this is an app's `main`.
pub fn serve<S: 'static>(provider: Provider<S>, ui: PanelUi<S>) -> io::Result<()> {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    serve_on(provider, ui, &mut reader, &mut writer)
}

/// [`serve`] over explicit streams — the seam the tests drive.
pub fn serve_on<S: 'static, R: BufRead, W: Write>(
    mut provider: Provider<S>,
    ui: PanelUi<S>,
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

    let mut state = provider.build(&init);

    // A `title` closure (if any) names the pane from the just-built state;
    // otherwise the static name stands.
    let PanelUi {
        name,
        title_fn,
        script,
        screens,
    } = ui;
    let name = match title_fn {
        Some(title) => title(&state),
        None => name,
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
        &Envelope::notification(method::SET_SCRIPT, SetScriptParams { source: script }),
    )?;

    // 3. Answer requests until shutdown / EOF.
    while let Some(env) = wire::read_message(reader)? {
        if env.is_method(method::QUERY) {
            let req_id = env.id.unwrap_or(0);
            let q: QueryParams = match env.params_as() {
                Ok(q) => q,
                Err(e) => {
                    eprintln!("petal-query: bad query params: {e}");
                    continue;
                }
            };
            let reply = provider.answer(
                &mut state,
                &crate::provider::QueryContext {
                    kind: &q.kind,
                    arg: &q.arg,
                    init: &init,
                },
            );
            let (value, error, policy) = reply.into_parts();
            let result = QueryResult {
                kind: q.kind,
                arg: q.arg,
                value,
                error,
                // Omit a forever policy so the wire is unchanged for the default.
                cache_control: (policy != CachePolicy::forever()).then_some(policy),
            };
            wire::write_message(writer, &Envelope::response(req_id, result))?;
        } else if env.is_method(method::EMIT) {
            let p: EmitParams = match env.params_as() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("petal-query: bad emit params: {e}");
                    continue;
                }
            };
            provider.handle_emit(
                &mut state,
                &crate::provider::EmitContext {
                    event: &p.event,
                    arg: &p.arg,
                    init: &init,
                },
            );
        } else if env.is_method(method::MUTATE) {
            let req_id = env.id.unwrap_or(0);
            let m: MutateParams = match env.params_as() {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("petal-query: bad mutate params: {e}");
                    continue;
                }
            };
            // An app-registered handler wins; otherwise the built-in `navigate`
            // serves a declared screen's source. An unregistered, non-navigate
            // mutation falls through to `provider.mutate`, which errors.
            let reply = if !provider.has_mutation(&m.name) && m.name == NAVIGATE {
                builtin_navigate(&screens, &m.arg)
            } else {
                provider.mutate(
                    &mut state,
                    &MutateContext {
                        name: &m.name,
                        arg: &m.arg,
                        init: &init,
                    },
                )
            };
            let (value, error, _policy) = reply.into_parts();
            wire::write_message(
                writer,
                &Envelope::response(req_id, MutateResult { name: m.name, value, error }),
            )?;
        } else if env.is_method(method::SHUTDOWN) {
            return Ok(());
        }
        // `resize`, `invalidate`, and unknown notifications need no action.
    }
    Ok(())
}

/// Answer the built-in [`NAVIGATE`] mutation: look the requested `screen` up in
/// the panel's declared `screens` and return its source. The declared set is the
/// allowlist — an undeclared screen is an error, so a script can only navigate to
/// screens the app shipped. `arg` is `{ "screen": name }`; on success the reply
/// value is `{ "screen": name, "source": <ptl source> }`.
fn builtin_navigate(screens: &HashMap<String, String>, arg: &serde_json::Value) -> Reply {
    let Some(screen) = arg.get("screen").and_then(|v| v.as_str()) else {
        return Reply::error("navigate: missing 'screen' argument".to_string());
    };
    match screens.get(screen) {
        Some(source) => Reply::json(serde_json::json!({ "screen": screen, "source": source })),
        None => Reply::error(format!("navigate: no such screen '{screen}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Reply;
    use serde_json::json;
    use std::io::Cursor;

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
            params: Some(
                serde_json::to_value(QueryParams {
                    kind: kind.into(),
                    arg: arg.into(),
                })
                .unwrap(),
            ),
            result: None,
            error: None,
        }
    }

    fn shutdown() -> Envelope {
        Envelope::notification(method::SHUTDOWN, json!({}))
    }

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
        serve_on(Provider::stateless(), PanelUi::new("demo", "SCRIPT"), &mut r, &mut w).unwrap();
        let msgs = output(&w);
        assert_eq!(msgs[0].result.as_ref().unwrap()["mode"], "panel");
        assert_eq!(msgs[0].result.as_ref().unwrap()["name"], "demo");
        assert!(msgs[1].is_method(method::SET_SCRIPT));
        assert_eq!(msgs[1].params.as_ref().unwrap()["source"], "SCRIPT");
    }

    #[test]
    fn title_from_state_names_the_pane() {
        let mut r = input(vec![init_req(), shutdown()]);
        let mut w: Vec<u8> = Vec::new();
        let provider = Provider::new(|init| init.repo_arg());
        let ui = PanelUi::new("static", "S").title(|repo: &String| format!("repo: {repo}"));
        serve_on(provider, ui, &mut r, &mut w).unwrap();
        assert_eq!(output(&w)[0].result.as_ref().unwrap()["name"], "repo: /repo");
    }

    #[test]
    fn dispatches_query_with_state_and_policy() {
        let mut r = input(vec![
            init_req(),
            query_req(5, "log", ""),
            query_req(6, "commit", "abc"),
            shutdown(),
        ]);
        let mut w: Vec<u8> = Vec::new();
        let provider = Provider::new(|init| init.args.first().cloned().unwrap_or_default())
            .query("log", |repo: &mut String, _ctx| {
                Reply::json(json!({ "repo": repo.clone() }))
                    .max_age(std::time::Duration::from_secs(3))
            })
            .query("commit", |_repo, ctx| Reply::json(json!({ "hash": ctx.arg })));
        serve_on(provider, PanelUi::new("git", "S"), &mut r, &mut w).unwrap();

        let msgs = output(&w);
        let log = msgs[2].result.as_ref().unwrap();
        assert_eq!(log["value"]["repo"], "/repo");
        assert_eq!(log["cacheControl"]["maxAgeMs"], 3000);
        let commit = msgs[3].result.as_ref().unwrap();
        assert_eq!(commit["value"]["hash"], "abc");
        assert!(commit.get("cacheControl").is_none());
    }

    fn mutate_req(id: u64, name: &str, arg: serde_json::Value) -> Envelope {
        Envelope {
            jsonrpc: "2.0".into(),
            id: Some(id),
            method: Some(method::MUTATE.into()),
            params: Some(serde_json::to_value(MutateParams { name: name.into(), arg }).unwrap()),
            result: None,
            error: None,
        }
    }

    #[test]
    fn builtin_navigate_serves_a_declared_screen_and_refuses_others() {
        let mut r = input(vec![
            init_req(),
            mutate_req(7, NAVIGATE, json!({ "screen": "b.ptl" })),
            mutate_req(8, NAVIGATE, json!({ "screen": "missing.ptl" })),
            shutdown(),
        ]);
        let mut w: Vec<u8> = Vec::new();
        let ui = PanelUi::new("home", "HOME").screen("b.ptl", "SOURCE_B");
        serve_on(Provider::stateless(), ui, &mut r, &mut w).unwrap();
        let msgs = output(&w);
        // The declared screen resolves to its source; an undeclared one errors.
        let ok = msgs[2].result.as_ref().unwrap();
        assert_eq!(ok["name"], "navigate");
        assert_eq!(ok["value"]["source"], "SOURCE_B");
        assert_eq!(ok["value"]["screen"], "b.ptl");
        assert!(ok.get("cacheControl").is_none(), "mutations are never cached");
        let err = msgs[3].result.as_ref().unwrap();
        assert!(err["error"].as_str().unwrap().contains("no such screen"));
    }

    #[test]
    fn app_registered_mutation_takes_precedence_and_is_effectful() {
        let mut r = input(vec![
            init_req(),
            mutate_req(9, "select", json!({ "row": 3 })),
            query_req(10, "selected", ""),
            shutdown(),
        ]);
        let mut w: Vec<u8> = Vec::new();
        // A mutation mutates state; a following query observes the effect.
        let provider = Provider::new(|_| 0i64)
            .on_mutation("select", |s: &mut i64, ctx| {
                *s = ctx.arg["row"].as_i64().unwrap_or(0);
                Reply::json(json!({ "ok": true }))
            })
            .query("selected", |s: &mut i64, _ctx| Reply::json(*s));
        serve_on(provider, PanelUi::new("demo", "S"), &mut r, &mut w).unwrap();
        let msgs = output(&w);
        assert_eq!(msgs[2].result.as_ref().unwrap()["value"]["ok"], true);
        assert_eq!(msgs[3].result.as_ref().unwrap()["value"], 3);
    }

    #[test]
    fn emit_reaches_its_handler() {
        let emit = Envelope::notification(
            method::EMIT,
            EmitParams {
                event: "ui_state".into(),
                arg: json!({ "left_frac": 300 }),
            },
        );
        let mut r = input(vec![init_req(), emit, query_req(2, "state", ""), shutdown()]);
        let mut w: Vec<u8> = Vec::new();
        let provider = Provider::new(|_| 0i64)
            .on_emit("ui_state", |s: &mut i64, ctx| {
                *s = ctx.arg["left_frac"].as_i64().unwrap_or(0);
            })
            .query("state", |s: &mut i64, _ctx| Reply::json(*s));
        serve_on(provider, PanelUi::new("demo", "S"), &mut r, &mut w).unwrap();
        assert_eq!(output(&w)[2].result.as_ref().unwrap()["value"], 300);
    }
}
