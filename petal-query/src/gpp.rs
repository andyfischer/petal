//! Garden Pane Protocol adapter: run a generic [`Provider`] as a GPP v2 client
//! app over stdio.
//!
//! This is where the **editor/presentation** concerns live — the pane name and
//! the UI script — kept out of the transport-agnostic [`Provider`]. A GPP
//! app is "a web server for one page": it names the pane, ships a Petal UI
//! script once (the page), then answers the `query(kind, arg)` calls that
//! script makes (the data), plus `mutate` / `navigate` requests and `emit`
//! notifications.
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
//!
//! The wire itself — envelope, message shapes, cache policy — is the `gpp`
//! crate; this module is the client-side protocol loop over it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};

use crate::provider::{
    EmitContext, MutateContext, NavigateContext, Provider, QueryContext, Reply,
};
use crate::CachePolicy;
use ::gpp::{
    error_code, method, EmitParams, Envelope, InitializeParams, InitializeResult, MutateParams,
    MutateResult, NavigateParams, NavigateResult, QueryParams, QueryResult, SetScriptParams,
    PROTOCOL_VERSION,
};

/// The capability names a `petal-query` app reports in its `initialize`
/// response: the requests it answers and the pushes it makes.
const CLIENT_CAPABILITIES: &[&str] = &["query", "mutate", "navigate", "emit", "setScript"];

/// The GPP presentation for an app: the pane name (static, or derived
/// from the built state) and the UI script the host runs. Supplied by the app —
/// the GPP layer — not by the transport-agnostic [`Provider`].
pub struct PanelUi<S> {
    name: String,
    title_fn: Option<Box<dyn FnOnce(&S) -> String>>,
    script: String,
    /// Additional navigable screens (name → UI source) beyond the home `script`.
    /// The declared set doubles as the navigation allowlist: a `navigate` request
    /// for an undeclared screen is refused. Empty means a single-screen app.
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
    /// can title the pane from what it just loaded (e.g. `db — <file>`).
    pub fn title(mut self, title: impl FnOnce(&S) -> String + 'static) -> PanelUi<S> {
        self.title_fn = Some(Box::new(title));
        self
    }

    /// Declare a navigable screen `name` served from `source`. The host's
    /// browser-style navigation (`navigate(name)`) fetches this source via a
    /// `navigate` request; the set of declared screens is the navigation
    /// allowlist. Fluent — chain one `.screen(...)` per screen.
    ///
    /// The home `script` from [`new`](Self::new) is screen 0 and need not be
    /// re-declared here (the host already holds it); declare the screens it can
    /// navigate *to*. An app that wants navigation effects (logging, priming
    /// data) registers [`Provider::on_navigate`] instead, which replaces this
    /// built-in lookup.
    pub fn screen(mut self, name: impl Into<String>, source: impl Into<String>) -> PanelUi<S> {
        self.screens.insert(name.into(), source.into());
        self
    }
}

/// One place a message goes out, **one whole envelope at a time**.
///
/// The atomicity is the point. `serde_json::to_writer` makes many small writes
/// per envelope, so a lock taken at the [`Write`] level would let a second
/// thread's message interleave *inside* a line — and the transport is one
/// compact JSON object per line. Every implementation here therefore serializes
/// a complete envelope under one lock.
trait Sink {
    fn send(&self, env: &Envelope) -> io::Result<()>;
}

/// The single-threaded sink: an exclusive borrow of the caller's writer. Used by
/// [`serve_on`], where nothing else can be writing.
struct PlainSink<'a, W: Write>(RefCell<&'a mut W>);

impl<W: Write> Sink for PlainSink<'_, W> {
    fn send(&self, env: &Envelope) -> io::Result<()> {
        gpp::write_message(&mut *self.0.borrow_mut(), env)
    }
}

/// A handle for pushing to the host **after** the handshake, from any thread.
///
/// The serve loop is otherwise the only writer, and it is blocked on the next
/// host message — so an app that wants to say something unprompted (a dev-mode
/// file watcher that has just seen a `.ptl` edit) needs a second way in. This is
/// it: cloneable, `Send`, and safe to hold across threads because every send is
/// one envelope under one lock.
///
/// Obtained from [`serve_with_reload`].
#[derive(Clone)]
pub struct ScriptSink {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl Sink for ScriptSink {
    fn send(&self, env: &Envelope) -> io::Result<()> {
        let mut w = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("gpp sink poisoned"))?;
        gpp::write_message(&mut *w, env)
    }
}

impl ScriptSink {
    /// A sink over an arbitrary writer. [`serve_with_reload`] builds one over
    /// stdout; the tests build one over a buffer they can read back.
    fn to_writer(writer: Box<dyn Write + Send>) -> ScriptSink {
        ScriptSink {
            writer: Arc::new(Mutex::new(writer)),
        }
    }

    /// Push a new UI script, replacing the one the panel is running.
    ///
    /// The host recompiles in place and **keeps its query cache**, so data the
    /// panel already fetched is not re-requested — a script edit costs a
    /// recompile, not a refetch. A source that fails to compile leaves the old
    /// program running and surfaces the error in the pane, so pushing
    /// mid-keystroke is safe.
    pub fn set_script(&self, source: impl Into<String>) -> io::Result<()> {
        self.send(&Envelope::notification(
            method::SET_SCRIPT,
            SetScriptParams {
                source: source.into(),
            },
        ))
    }
}

/// Run `provider` as a GPP client app on stdio until `shutdown` / EOF,
/// presenting it with `ui` (pane name + UI script). Blocks the calling thread;
/// this is an app's `main`.
pub fn serve<S: 'static>(provider: Provider<S>, ui: PanelUi<S>) -> io::Result<()> {
    serve_with_reload(provider, ui, |_| {})
}

/// [`serve`], handing the app a [`ScriptSink`] once the panel is up.
///
/// `on_ready` is called after the initial script has been pushed, with a sink
/// the app can move into a background thread — the hot-reload seam. It must not
/// block: the serve loop does not start answering queries until it returns.
///
/// ```no_run
/// # use petal_query::{Provider, gpp::{self, PanelUi}};
/// # fn bundle() -> String { String::new() }
/// # let (provider, ui): (Provider<()>, PanelUi<()>) = (Provider::new(|_| ()), PanelUi::new("x", ""));
/// gpp::serve_with_reload(provider, ui, |sink| {
///     std::thread::spawn(move || loop {
///         // …wait for a source change, then:
///         let _ = sink.set_script(bundle());
///     });
/// })?;
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn serve_with_reload<S: 'static>(
    provider: Provider<S>,
    ui: PanelUi<S>,
    on_ready: impl FnOnce(ScriptSink),
) -> io::Result<()> {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());
    let sink = ScriptSink::to_writer(Box::new(io::stdout()));
    let handed = sink.clone();
    serve_core(provider, ui, &mut reader, &sink, move |_| on_ready(handed))
}

/// [`serve`] over explicit streams — the seam the tests drive.
pub fn serve_on<S: 'static, R: BufRead, W: Write>(
    provider: Provider<S>,
    ui: PanelUi<S>,
    reader: &mut R,
    writer: &mut W,
) -> io::Result<()> {
    let sink = PlainSink(RefCell::new(writer));
    serve_core(provider, ui, reader, &sink, |_| {})
}

/// Map a handler [`Reply`] onto the wire: a value becomes a success response
/// carrying it (with the cache policy unless it is the default), an error an
/// [`error_code::APP`] error response, and "still loading" an empty result.
fn query_response(id: u64, reply: Reply) -> Envelope {
    let (value, error, policy) = reply.into_parts();
    match error {
        Some(message) => Envelope::error_response(id, error_code::APP, message),
        None => Envelope::response(
            id,
            QueryResult {
                value,
                // Omit a forever policy so the default adds nothing to the wire.
                cache: (policy != CachePolicy::forever()).then_some(policy),
            },
        ),
    }
}

/// The protocol loop, over any [`Sink`]. `on_ready` runs once the panel's script
/// has been pushed.
fn serve_core<S: 'static, R: BufRead, K: Sink>(
    mut provider: Provider<S>,
    ui: PanelUi<S>,
    reader: &mut R,
    writer: &K,
    on_ready: impl FnOnce(&K),
) -> io::Result<()> {
    // 1. Handshake: read `initialize`, check the protocol version, build state,
    //    reply.
    let init_env = match gpp::read_message(reader)? {
        Some(env) if env.is_method(method::INITIALIZE) => env,
        _ => return Ok(()), // EOF or unexpected first message
    };
    let id = init_env.id.unwrap_or(1);
    let init: InitializeParams = match init_env.params_as() {
        Ok(init) => init,
        Err(e) => {
            writer.send(&Envelope::error_response(
                id,
                error_code::INVALID_PARAMS,
                format!("bad initialize params: {e}"),
            ))?;
            return Ok(());
        }
    };
    if init.protocol != PROTOCOL_VERSION {
        // A major mismatch ends the session cleanly: the host reads this error
        // and surfaces it in the pane instead of a wedged panel.
        writer.send(&Envelope::error_response(
            id,
            error_code::PROTOCOL_MISMATCH,
            format!(
                "this app speaks gpp protocol {PROTOCOL_VERSION}, the host sent {}",
                init.protocol
            ),
        ))?;
        return Ok(());
    }

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

    writer.send(&Envelope::response(
        id,
        InitializeResult {
            protocol: PROTOCOL_VERSION,
            name,
            capabilities: CLIENT_CAPABILITIES.iter().map(|c| c.to_string()).collect(),
        },
    ))?;

    // 2. Push the UI script; the host compiles it into a panel.
    writer.send(&Envelope::notification(
        method::SET_SCRIPT,
        SetScriptParams { source: script },
    ))?;

    // The panel is up, so a hot-reload watcher can start pushing at any time.
    on_ready(writer);

    // 3. Answer requests until shutdown / EOF.
    while let Some(env) = gpp::read_message(reader)? {
        if env.is_method(method::QUERY) {
            let req_id = env.id.unwrap_or(0);
            let q: QueryParams = match env.params_as() {
                Ok(q) => q,
                Err(e) => {
                    writer.send(&Envelope::error_response(
                        req_id,
                        error_code::INVALID_PARAMS,
                        format!("bad query params: {e}"),
                    ))?;
                    continue;
                }
            };
            let reply = provider.answer(
                &mut state,
                &QueryContext {
                    kind: &q.kind,
                    arg: &q.arg,
                    init: &init,
                },
            );
            writer.send(&query_response(req_id, reply))?;
        } else if env.is_method(method::MUTATE) {
            let req_id = env.id.unwrap_or(0);
            let m: MutateParams = match env.params_as() {
                Ok(m) => m,
                Err(e) => {
                    writer.send(&Envelope::error_response(
                        req_id,
                        error_code::INVALID_PARAMS,
                        format!("bad mutate params: {e}"),
                    ))?;
                    continue;
                }
            };
            let reply = provider.mutate(
                &mut state,
                &MutateContext {
                    name: &m.name,
                    arg: &m.arg,
                    init: &init,
                },
            );
            let (value, error, _policy) = reply.into_parts();
            let response = match error {
                Some(message) => Envelope::error_response(req_id, error_code::APP, message),
                None => Envelope::response(req_id, MutateResult { value }),
            };
            writer.send(&response)?;
        } else if env.is_method(method::NAVIGATE) {
            let req_id = env.id.unwrap_or(0);
            let n: NavigateParams = match env.params_as() {
                Ok(n) => n,
                Err(e) => {
                    writer.send(&Envelope::error_response(
                        req_id,
                        error_code::INVALID_PARAMS,
                        format!("bad navigate params: {e}"),
                    ))?;
                    continue;
                }
            };
            // A registered on_navigate handler wins (side effects + source);
            // otherwise the built-in serves a declared screen. The declared set
            // is the allowlist: an undeclared screen is refused.
            let outcome = provider
                .navigate(
                    &mut state,
                    &NavigateContext {
                        screen: &n.screen,
                        arg: &n.arg,
                        init: &init,
                    },
                )
                .unwrap_or_else(|| match screens.get(&n.screen) {
                    Some(source) => Ok(source.clone()),
                    None => Err(format!("no such screen '{}'", n.screen)),
                });
            let response = match outcome {
                Ok(source) => Envelope::response(
                    req_id,
                    NavigateResult {
                        screen: n.screen,
                        source,
                    },
                ),
                Err(message) => Envelope::error_response(req_id, error_code::APP, message),
            };
            writer.send(&response)?;
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
                &EmitContext {
                    event: &p.event,
                    arg: &p.arg,
                    init: &init,
                },
            );
        } else if env.is_method(method::SHUTDOWN) {
            return Ok(());
        } else if let (Some(req_id), Some(m)) = (env.id, env.method.as_deref()) {
            // An unknown *request* deserves an answer, or the host would wait
            // out its timeout; unknown notifications are silently skipped.
            writer.send(&Envelope::error_response(
                req_id,
                error_code::METHOD_NOT_FOUND,
                format!("unknown method '{m}'"),
            ))?;
        }
    }
    Ok(())
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
            gpp::write_message(&mut buf, env).unwrap();
        }
        Cursor::new(buf)
    }

    fn init_req() -> Envelope {
        Envelope::request(
            1,
            method::INITIALIZE,
            InitializeParams {
                protocol: PROTOCOL_VERSION,
                pane_id: 0,
                rows: 40,
                cols: 120,
                args: vec!["/repo".to_string()],
                cwd: "/repo".to_string(),
                capabilities: vec![],
            },
        )
    }

    fn query_req(id: u64, kind: &str, arg: serde_json::Value) -> Envelope {
        Envelope::request(
            id,
            method::QUERY,
            QueryParams {
                kind: kind.into(),
                arg,
            },
        )
    }

    fn mutate_req(id: u64, name: &str, arg: serde_json::Value) -> Envelope {
        Envelope::request(
            id,
            method::MUTATE,
            MutateParams {
                name: name.into(),
                arg,
            },
        )
    }

    fn navigate_req(id: u64, screen: &str, arg: serde_json::Value) -> Envelope {
        Envelope::request(
            id,
            method::NAVIGATE,
            NavigateParams {
                screen: screen.into(),
                arg,
            },
        )
    }

    fn shutdown() -> Envelope {
        Envelope::notification(method::SHUTDOWN, json!({}))
    }

    fn output(buf: &[u8]) -> Vec<Envelope> {
        let mut reader = std::io::BufReader::new(buf);
        let mut out = Vec::new();
        while let Some(env) = gpp::read_message(&mut reader).unwrap() {
            out.push(env);
        }
        out
    }

    /// The response with `id` in the output, panicking when absent.
    fn response_with_id(msgs: &[Envelope], id: u64) -> &Envelope {
        msgs.iter()
            .find(|e| e.is_response() && e.id == Some(id))
            .unwrap_or_else(|| panic!("no response with id {id}"))
    }

    #[test]
    fn handshake_reports_protocol_2_and_pushes_the_script() {
        let mut r = input(vec![init_req(), shutdown()]);
        let mut w: Vec<u8> = Vec::new();
        serve_on(
            Provider::stateless(),
            PanelUi::new("demo", "SCRIPT"),
            &mut r,
            &mut w,
        )
        .unwrap();
        let msgs = output(&w);
        let init = msgs[0].result.as_ref().unwrap();
        assert_eq!(init["protocol"], 2);
        assert_eq!(init["name"], "demo");
        assert!(init["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "query"));
        assert!(msgs[1].is_method(method::SET_SCRIPT));
        assert_eq!(msgs[1].params.as_ref().unwrap()["source"], "SCRIPT");
    }

    #[test]
    fn a_protocol_mismatch_is_refused_with_a_clean_error() {
        let old = Envelope::request(
            1,
            method::INITIALIZE,
            json!({ "paneId": 0, "rows": 5, "cols": 5, "args": [], "cwd": "." }),
        );
        let mut r = input(vec![old]);
        let mut w: Vec<u8> = Vec::new();
        serve_on(
            Provider::stateless(),
            PanelUi::new("demo", "S"),
            &mut r,
            &mut w,
        )
        .unwrap();
        let msgs = output(&w);
        assert_eq!(msgs.len(), 1, "no script push after a refused handshake");
        let err = msgs[0].error.as_ref().unwrap();
        assert_eq!(err.code, error_code::PROTOCOL_MISMATCH);
        assert!(err.message.contains("protocol 2"));
    }

    #[test]
    fn title_from_state_names_the_pane() {
        let mut r = input(vec![init_req(), shutdown()]);
        let mut w: Vec<u8> = Vec::new();
        let provider = Provider::new(|init| init.repo_arg());
        let ui = PanelUi::new("static", "S").title(|repo: &String| format!("repo: {repo}"));
        serve_on(provider, ui, &mut r, &mut w).unwrap();
        assert_eq!(
            output(&w)[0].result.as_ref().unwrap()["name"],
            "repo: /repo"
        );
    }

    #[test]
    fn queries_answer_by_id_with_json_args_and_policies() {
        let mut r = input(vec![
            init_req(),
            query_req(5, "log", json!("")),
            query_req(6, "table", json!({ "name": "users", "page": 2 })),
            shutdown(),
        ]);
        let mut w: Vec<u8> = Vec::new();
        let provider = Provider::new(|init| init.args.first().cloned().unwrap_or_default())
            .query("log", |repo: &mut String, _ctx| {
                Reply::json(json!({ "repo": repo.clone() }))
                    .max_age(std::time::Duration::from_secs(3))
            })
            .query("table", |_repo, ctx| {
                Reply::json(json!({ "page": ctx.arg["page"] }))
            });
        serve_on(provider, PanelUi::new("git", "S"), &mut r, &mut w).unwrap();

        let msgs = output(&w);
        let log = response_with_id(&msgs, 5).result.as_ref().unwrap();
        assert_eq!(log["value"]["repo"], "/repo");
        assert_eq!(log["cache"]["maxAgeMs"], 3000);
        // No kind/arg echo anywhere in the result — the id is the correlation.
        assert!(log.get("kind").is_none());
        assert!(log.get("arg").is_none());
        let table = response_with_id(&msgs, 6).result.as_ref().unwrap();
        assert_eq!(table["value"]["page"], 2);
        assert!(table.get("cache").is_none(), "forever adds nothing");
    }

    #[test]
    fn a_failed_query_is_an_error_response() {
        let mut r = input(vec![init_req(), query_req(4, "boom", json!("")), shutdown()]);
        let mut w: Vec<u8> = Vec::new();
        let provider =
            Provider::stateless().query("boom", |_s, _ctx| Reply::error("upstream failed"));
        serve_on(provider, PanelUi::new("demo", "S"), &mut r, &mut w).unwrap();
        let resp = output(&w)
            .into_iter()
            .find(|e| e.id == Some(4))
            .expect("a response for id 4");
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, error_code::APP);
        assert_eq!(err.message, "upstream failed");
    }

    #[test]
    fn a_loading_reply_is_an_empty_result() {
        let mut r = input(vec![init_req(), query_req(4, "slow", json!("")), shutdown()]);
        let mut w: Vec<u8> = Vec::new();
        let provider = Provider::stateless().query("slow", |_s, _ctx| Reply::loading());
        serve_on(provider, PanelUi::new("demo", "S"), &mut r, &mut w).unwrap();
        let msgs = output(&w);
        let resp = response_with_id(&msgs, 4);
        assert_eq!(resp.result.as_ref().unwrap(), &json!({}));
    }

    #[test]
    fn navigate_serves_a_declared_screen_and_refuses_others() {
        let mut r = input(vec![
            init_req(),
            navigate_req(7, "b.ptl", json!(null)),
            navigate_req(8, "missing.ptl", json!(null)),
            shutdown(),
        ]);
        let mut w: Vec<u8> = Vec::new();
        let ui = PanelUi::new("home", "HOME").screen("b.ptl", "SOURCE_B");
        serve_on(Provider::stateless(), ui, &mut r, &mut w).unwrap();
        let msgs = output(&w);
        let ok = response_with_id(&msgs, 7).result.as_ref().unwrap();
        assert_eq!(ok["screen"], "b.ptl");
        assert_eq!(ok["source"], "SOURCE_B");
        let refused = msgs.iter().find(|e| e.id == Some(8)).unwrap();
        let err = refused.error.as_ref().unwrap();
        assert_eq!(err.code, error_code::APP);
        assert!(err.message.contains("no such screen"));
    }

    #[test]
    fn a_custom_navigate_handler_takes_precedence_and_sees_the_arg() {
        let mut r = input(vec![
            init_req(),
            navigate_req(7, "detail.ptl", json!({ "id": 9 })),
            shutdown(),
        ]);
        let mut w: Vec<u8> = Vec::new();
        let provider = Provider::new(|_| 0i64).on_navigate(|visits: &mut i64, ctx| {
            *visits += 1;
            Ok(format!("// {} for id {} visit {visits}", ctx.screen, ctx.arg["id"]))
        });
        // The declared screen would say something else; the handler wins.
        let ui = PanelUi::new("home", "HOME").screen("detail.ptl", "DECLARED");
        serve_on(provider, ui, &mut r, &mut w).unwrap();
        let msgs = output(&w);
        let ok = response_with_id(&msgs, 7).result.as_ref().unwrap();
        assert_eq!(ok["source"], "// detail.ptl for id 9 visit 1");
    }

    #[test]
    fn mutations_dispatch_are_effectful_and_error_by_envelope() {
        let mut r = input(vec![
            init_req(),
            mutate_req(9, "select", json!({ "row": 3 })),
            query_req(10, "selected", json!("")),
            mutate_req(11, "unknown", json!(null)),
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
        assert_eq!(
            response_with_id(&msgs, 9).result.as_ref().unwrap()["value"]["ok"],
            true
        );
        assert_eq!(
            response_with_id(&msgs, 10).result.as_ref().unwrap()["value"],
            3
        );
        let unknown = msgs.iter().find(|e| e.id == Some(11)).unwrap();
        let err = unknown.error.as_ref().unwrap();
        assert_eq!(err.code, error_code::APP);
        assert!(err.message.contains("no mutation handler"));
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
        let mut r = input(vec![init_req(), emit, query_req(2, "state", json!("")), shutdown()]);
        let mut w: Vec<u8> = Vec::new();
        let provider = Provider::new(|_| 0i64)
            .on_emit("ui_state", |s: &mut i64, ctx| {
                *s = ctx.arg["left_frac"].as_i64().unwrap_or(0);
            })
            .query("state", |s: &mut i64, _ctx| Reply::json(*s));
        serve_on(provider, PanelUi::new("demo", "S"), &mut r, &mut w).unwrap();
        let msgs = output(&w);
        assert_eq!(
            response_with_id(&msgs, 2).result.as_ref().unwrap()["value"],
            300
        );
    }

    #[test]
    fn an_unknown_request_gets_method_not_found() {
        let mut r = input(vec![
            init_req(),
            Envelope::request(12, "futureThing", json!({})),
            Envelope::notification("futureNotification", json!({})),
            shutdown(),
        ]);
        let mut w: Vec<u8> = Vec::new();
        serve_on(
            Provider::stateless(),
            PanelUi::new("demo", "S"),
            &mut r,
            &mut w,
        )
        .unwrap();
        let msgs = output(&w);
        let unknown = msgs.iter().find(|e| e.id == Some(12)).unwrap();
        assert_eq!(
            unknown.error.as_ref().unwrap().code,
            error_code::METHOD_NOT_FOUND
        );
        // The unknown notification was skipped without an answer: only the
        // initialize response, the script push, and the one error went out.
        assert_eq!(msgs.len(), 3);
    }

    /// A writer several threads can hold, so a test can read what the sink and
    /// the serve loop both wrote.
    #[derive(Clone)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_sink_push_replaces_the_script_after_the_handshake() {
        let buf = SharedBuf(Arc::new(Mutex::new(Vec::new())));
        let sink = ScriptSink::to_writer(Box::new(buf.clone()));
        let mut r = input(vec![init_req(), shutdown()]);
        let provider: Provider<()> = Provider::new(|_| ());
        serve_core(
            provider,
            PanelUi::new("demo", "FIRST"),
            &mut r,
            &sink,
            |s| s.set_script("SECOND").unwrap(),
        )
        .unwrap();

        let msgs = output(&buf.0.lock().unwrap());
        // initialize response, the handshake's script, then the pushed one.
        assert!(msgs[1].is_method(method::SET_SCRIPT));
        assert_eq!(msgs[1].params.as_ref().unwrap()["source"], "FIRST");
        assert!(msgs[2].is_method(method::SET_SCRIPT));
        assert_eq!(msgs[2].params.as_ref().unwrap()["source"], "SECOND");
    }

    /// The reason [`Sink`] serializes whole envelopes rather than locking at the
    /// [`Write`] level: two threads pushing at once must not interleave inside a
    /// line, because the transport is one JSON object per line.
    #[test]
    fn concurrent_sink_pushes_stay_one_object_per_line() {
        let buf = SharedBuf(Arc::new(Mutex::new(Vec::new())));
        let sink = ScriptSink::to_writer(Box::new(buf.clone()));
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let s = sink.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        s.set_script(format!("script-{i}")).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        // Every line parses, and all 200 arrived.
        let msgs = output(&buf.0.lock().unwrap());
        assert_eq!(msgs.len(), 200);
        assert!(msgs.iter().all(|m| m.is_method(method::SET_SCRIPT)));
    }
}
