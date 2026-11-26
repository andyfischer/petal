//! Placeholder passes end to end: a fake site whose pages a native answers,
//! a placeholder while a page is not fetched, a failed query answer once its
//! fetch failed; and the settle loop over scripts that fan out, chain, fail
//! on stand-ins, fail for real, and run into the run cap.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use petal::env::Env;
use petal::native_fn::{InputClasses, NativeEffects};
use petal::stack::StackKey;
use petal::value::{Value, json_to_value, value_to_json};
use petal_batch::{
    BatchHost, Resolution, RunReport, is_speculative, note_placeholder, push_placeholder, settle,
    settle_with, take_placeholders,
};
use serde_json::{Value as Json, json};

/// A fake site: `pages` is what exists, `have` what the host has fetched
/// (what `fetch` answers from), `fetched` every fetch in order.
#[derive(Default)]
struct Site {
    pages: HashMap<String, Json>,
    have: HashMap<String, Result<Json, String>>,
    fetched: Vec<String>,
    /// Whether the last `fetch` call was made while the run was already
    /// speculative, per call: what `is_speculative` said.
    speculative_seen: Vec<bool>,
}

impl Site {
    /// Fetch every name handed over: a page that exists resolves, any other
    /// fails with a 404.
    fn resolve(&mut self, names: &[String]) -> Vec<(String, Resolution)> {
        names
            .iter()
            .map(|url| {
                self.fetched.push(url.clone());
                let (have, resolution) = match self.pages.get(url) {
                    Some(page) => (Ok(page.clone()), Resolution::Resolved),
                    None => {
                        let msg = format!("404 {url}");
                        (Err(msg.clone()), Resolution::Errored(msg))
                    }
                };
                self.have.insert(url.clone(), have);
                (url.clone(), resolution)
            })
            .collect()
    }
}

/// The stand-in for a page not fetched yet: a page record's shape, empty.
fn placeholder(url: &str) -> Json {
    json!({"url": url, "status": 0, "title": "", "next": null, "links": [], "count": null})
}

const PRELUDE: &str = "import io\nimport query\nnative fn fetch(url)\nnative fn get_raw(url)\nnative fn load(key)\n";

/// An `Env` running `script` (after [`PRELUDE`]) against `pages`, with:
/// - `fetch(url)`: the page, a failed query answer, or a placeholder
///   ([`push_placeholder`]);
/// - `get_raw(url)`: the same, but the native pushes the stand-in itself
///   after [`note_placeholder`];
/// - `load(key)`: a `query`-package loading answer until resolved.
fn host(pages: &[(&str, Json)], script: &str) -> (Env, StackKey, Rc<RefCell<Site>>) {
    let site = Rc::new(RefCell::new(Site {
        pages: pages.iter().map(|(u, p)| (u.to_string(), p.clone())).collect(),
        ..Site::default()
    }));
    let mut env = Env::new();
    petal_stdlib_query::register(&mut env);
    env.set_echo(false);
    let reads = NativeEffects::reads(InputClasses::HOST_DATA);

    let s = site.clone();
    env.register_native_boxed(
        "fetch",
        move |cx| {
            let url = match cx.get_value(1)? {
                Value::String(id) => cx.heap().get_string(id).to_string(),
                other => return Err(format!("fetch wants a url, got {}", other.type_name())),
            };
            s.borrow_mut().speculative_seen.push(is_speculative());
            let have = s.borrow().have.get(&url).cloned();
            match have {
                Some(Ok(page)) => {
                    let v = json_to_value(&page, cx.heap_mut()).map_err(|e| e.to_string())?;
                    cx.note_host_read();
                    cx.push_value(v);
                }
                Some(Err(msg)) => petal_stdlib_query::push_failed(cx, &url, msg)?,
                None => {
                    let v = json_to_value(&placeholder(&url), cx.heap_mut()).map_err(|e| e.to_string())?;
                    push_placeholder(cx, &url, v)?;
                }
            }
            Ok(1)
        },
        reads,
    );

    let s = site.clone();
    env.register_native_boxed(
        "get_raw",
        move |cx| {
            let url = cx.get_string(1)?;
            let have = s.borrow().have.get(&url).cloned();
            match have {
                Some(Ok(page)) => {
                    let v = json_to_value(&page, cx.heap_mut()).map_err(|e| e.to_string())?;
                    cx.push_value(v);
                }
                Some(Err(msg)) => petal_stdlib_query::push_failed(cx, &url, msg)?,
                None => {
                    note_placeholder(&url);
                    cx.note_host_read();
                    cx.push_nil();
                }
            }
            Ok(1)
        },
        reads,
    );

    let s = site.clone();
    env.register_native_boxed(
        "load",
        move |cx| {
            let key = cx.get_string(1)?;
            let have = s.borrow().have.get(&key).cloned();
            match have {
                Some(Ok(page)) => {
                    let v = json_to_value(&page, cx.heap_mut()).map_err(|e| e.to_string())?;
                    cx.push_value(v);
                }
                Some(Err(msg)) => petal_stdlib_query::push_failed(cx, &key, msg)?,
                None => petal_stdlib_query::push_loading(cx, &key)?,
            }
            Ok(1)
        },
        reads,
    );

    let pid = env
        .load_program(&format!("{PRELUDE}{script}"))
        .unwrap_or_else(|e| panic!("{e}"));
    let stack = env.create_stack(pid).unwrap();
    (env, stack, site)
}

fn drain(env: &mut Env, channel: &str) -> Json {
    let sym = env.intern_symbol(channel);
    let values = env.take_output_buffer(sym);
    Json::Array(values.iter().map(|v| value_to_json(v, env.heap()).unwrap()).collect())
}

fn names(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

fn speculative(runs: &[RunReport]) -> Vec<bool> {
    runs.iter().map(|r| r.speculative).collect()
}

// ── Breadth ──────────────────────────────────────────────────────────────

/// A list page linking N detail pages: run 0 asks for the list, run 1 reads
/// the real list and hands out N placeholders, so all N are fetched in one
/// batch, and run 2 uses only real data.
#[test]
fn a_loop_over_n_placeholders_asks_for_all_n_in_one_run() {
    const N: usize = 25;
    let links: Vec<String> = (0..N).map(|i| format!("/d/{i}")).collect();
    let mut pages = vec![("/list".to_string(), json!({"links": links}))];
    for (i, l) in links.iter().enumerate() {
        pages.push((l.clone(), json!({"title": format!("T{i}")})));
    }
    let pages: Vec<(&str, Json)> = pages.iter().map(|(u, p)| (u.as_str(), p.clone())).collect();
    let script = "\
let list = fetch(\"/list\")
for u in list.links do
  let d = fetch(u)
  push_output(symbol(\"rows\"), d.title)
end
\"done\"
";
    let (mut env, stack, site) = host(&pages, script);
    let report = settle(&mut env, stack, 10, |_, names| site.borrow_mut().resolve(names)).unwrap();

    assert!(report.settled, "{report:?}");
    assert_eq!(report.runs.len(), 3, "{report:?}");
    assert_eq!(speculative(&report.runs), [true, true, false]);
    assert_eq!(report.runs[0].requested, names(&["/list"]));
    assert_eq!(report.runs[1].requested, links, "every detail page in one run");
    assert_eq!(report.runs[1].resolved, links);
    assert!(report.runs[2].requested.is_empty() && report.runs[2].placeholders.is_empty());
    assert!(report.runs.iter().all(|r| r.error.is_none()), "{report:?}");
    assert_eq!(site.borrow().fetched.len(), N + 1, "each page fetched once");
    assert_eq!(report.error(), None);

    let value = report.value.expect("the settled run's value");
    assert_eq!(value_to_json(&value, env.heap()).unwrap(), json!("done"));
    let rows = drain(&mut env, "rows");
    let want: Vec<Json> = (0..N).map(|i| json!(format!("T{i}"))).collect();
    assert_eq!(rows, Json::Array(want), "only the settled run's emits");
}

/// Two calls for the same page in one run are one request.
#[test]
fn a_name_asked_twice_in_a_run_is_one_request() {
    let script = "let a = fetch(\"/a\")\nlet b = fetch(\"/a\")\nprint(a.title, b.title)\n";
    let (mut env, stack, site) = host(&[("/a", json!({"title": "A"}))], script);
    let report = settle(&mut env, stack, 10, |_, names| site.borrow_mut().resolve(names)).unwrap();
    assert!(report.settled);
    assert_eq!(report.runs[0].placeholders, names(&["/a"]));
    assert_eq!(site.borrow().fetched, names(&["/a"]));
    assert_eq!(env.take_output(), ["A A"]);
}

// ── Waterfalls ───────────────────────────────────────────────────────────

const CHAIN: &str = "\
let a = fetch(\"/a\")
let b = fetch(a.next)
let c = fetch(b.next)
let d = fetch(c.next)
print(a.title, b.title, c.title, d.title)
";

fn chain_pages() -> Vec<(&'static str, Json)> {
    vec![
        ("/a", json!({"title": "A", "next": "/b"})),
        ("/b", json!({"title": "B", "next": "/c"})),
        ("/c", json!({"title": "C", "next": "/d"})),
        ("/d", json!({"title": "D", "next": null})),
    ]
}

/// Each level of a chain is known only once the level above is real: four
/// levels cost four speculative runs, then the settled one. The stand-in's
/// `next` is nil, so each speculative run stops at the next `fetch(nil)`: a
/// tolerated error.
#[test]
fn each_waterfall_level_costs_one_run() {
    let (mut env, stack, site) = host(&chain_pages(), CHAIN);
    let report = settle(&mut env, stack, 10, |_, names| site.borrow_mut().resolve(names)).unwrap();

    assert!(report.settled, "{report:?}");
    assert_eq!(report.runs.len(), 5, "{report:?}");
    assert_eq!(speculative(&report.runs), [true, true, true, true, false]);
    let requested: Vec<Vec<String>> = report.runs.iter().map(|r| r.requested.clone()).collect();
    assert_eq!(
        requested,
        [names(&["/a"]), names(&["/b"]), names(&["/c"]), names(&["/d"]), vec![]]
    );
    for r in &report.runs[..3] {
        let e = r.error.as_deref().expect("fetch(nil) on a stand-in fails");
        assert!(e.contains("fetch wants a url"), "{e}");
    }
    assert!(report.runs[3].error.is_none(), "d's stand-in is only printed");
    assert_eq!(report.error(), None);
    assert_eq!(site.borrow().fetched, names(&["/a", "/b", "/c", "/d"]));
    assert_eq!(env.take_output(), ["A B C D"], "only the settled run's print");
}

// ── Errors ───────────────────────────────────────────────────────────────

/// Arithmetic on a stand-in's nil field fails in the speculative run; the
/// error is tolerated, and the settled run, on the real page, succeeds.
#[test]
fn an_error_in_a_speculative_run_is_tolerated() {
    let script = "let p = fetch(\"/a\")\nlet n = p.count + 1\nprint(n)\nn\n";
    let (mut env, stack, site) = host(&[("/a", json!({"count": 41}))], script);
    let report = settle(&mut env, stack, 10, |_, names| site.borrow_mut().resolve(names)).unwrap();

    assert!(report.settled, "{report:?}");
    assert_eq!(report.runs.len(), 2);
    assert!(report.runs[0].speculative && report.runs[0].error.is_some(), "{report:?}");
    assert_eq!(report.error(), None);
    assert_eq!(report.value, Some(Value::Int(42)));
    assert_eq!(env.take_output(), ["42"]);
}

/// The settled run used only real data, so its error is a real one: the
/// report carries it and has no value.
#[test]
fn an_error_on_the_settled_run_is_reported() {
    let script = "let p = fetch(\"/a\")\nprint(p.title)\nlet n = p.count + 1\nn\n";
    let (mut env, stack, site) = host(&[("/a", json!({"title": "A"}))], script);
    let report = settle(&mut env, stack, 10, |_, names| site.borrow_mut().resolve(names)).unwrap();

    assert!(report.settled, "{report:?}");
    assert_eq!(report.runs.len(), 2);
    assert!(!report.last().speculative);
    let e = report.error().expect("the real page has no count");
    assert_eq!(Some(e), report.last().error.as_deref());
    assert_eq!(report.value, None);
    assert_eq!(env.take_output(), ["A"], "what the failing run printed before it failed");
}

/// A failed fetch is a failed query answer in the next run, readable with
/// `is_error` / `error_of`, and not a placeholder: the loop settles.
#[test]
fn a_failed_fetch_is_a_failed_handle_the_script_can_test() {
    let script = "\
for u in [\"/a\", \"/missing\"] do
  let p = fetch(u)
  if is_error(p) then
    push_output(symbol(\"errors\"), error_of(p))
  else
    push_output(symbol(\"rows\"), p.title)
  end
end
";
    let (mut env, stack, site) = host(&[("/a", json!({"title": "A"}))], script);
    let report = settle(&mut env, stack, 10, |_, names| site.borrow_mut().resolve(names)).unwrap();

    assert!(report.settled, "{report:?}");
    assert_eq!(report.runs.len(), 2);
    assert_eq!(report.runs[0].resolved, names(&["/a"]));
    assert_eq!(report.runs[0].errored, vec![("/missing".to_string(), "404 /missing".to_string())]);
    assert!(!report.runs[1].speculative);
    assert_eq!(drain(&mut env, "rows"), json!(["A"]));
    assert_eq!(drain(&mut env, "errors"), json!(["404 /missing"]));
}

/// Using a failed answer as data is an error on the settled run, naming it.
#[test]
fn using_a_failed_handle_as_data_is_a_real_error() {
    let (mut env, stack, site) = host(&[], "let p = fetch(\"/gone\")\nprint(p.title)\n");
    let report = settle(&mut env, stack, 10, |_, names| site.borrow_mut().resolve(names)).unwrap();
    assert!(report.settled);
    let e = report.error().expect("a field read of a failed answer fails");
    assert!(e.contains("handle") || e.contains("pending answer"), "{e}");
}

// ── Stopping ─────────────────────────────────────────────────────────────

/// A run cap that comes first: the last run's new names are reported but not
/// resolved, the loop has not settled, and the report gives no error.
#[test]
fn max_runs_caps_the_loop() {
    let (mut env, stack, site) = host(&chain_pages(), CHAIN);
    let report = settle(&mut env, stack, 2, |_, names| site.borrow_mut().resolve(names)).unwrap();

    assert!(!report.settled);
    assert_eq!(report.runs.len(), 2);
    assert!(report.last().speculative);
    assert_eq!(report.last().requested, names(&["/b"]));
    assert!(report.last().resolved.is_empty());
    assert_eq!(report.unanswered, names(&["/b"]));
    assert_eq!(report.error(), None, "a speculative run's error is not the loop's");
    assert!(report.last().error.is_some());
    assert_eq!(site.borrow().fetched, names(&["/a"]));
}

/// `max_runs` of 0 still runs once.
#[test]
fn max_runs_zero_runs_once() {
    let (mut env, stack, _site) = host(&[], "print(1)\n");
    let report = settle(&mut env, stack, 0, |_, _| panic!("nothing to resolve")).unwrap();
    assert!(report.settled);
    assert_eq!(report.runs.len(), 1);
    assert_eq!(env.take_output(), ["1"]);
}

/// A host that leaves a name unresolved: the next run hands out the same
/// placeholder, which is nothing new, so the loop stops there unsettled.
#[test]
fn a_name_the_host_leaves_unresolved_stops_the_loop() {
    let (mut env, stack, _site) = host(&chain_pages(), CHAIN);
    let report = settle(&mut env, stack, 10, |_, _| Vec::new()).unwrap();
    assert!(!report.settled);
    assert_eq!(report.runs.len(), 2);
    assert_eq!(report.runs[0].requested, names(&["/a"]));
    assert!(report.runs[1].requested.is_empty());
    assert_eq!(report.unanswered, names(&["/a"]));
}

// ── Outputs, state and hooks ─────────────────────────────────────────────

/// A host that records each run's outputs from `after_run`, before the next
/// run drops them.
#[derive(Default)]
struct Recorder {
    site: Rc<RefCell<Site>>,
    before: Vec<usize>,
    /// Per run: (speculative, print output, `rows` buffer).
    seen: Vec<(bool, Vec<String>, Json)>,
}

impl BatchHost for Recorder {
    fn resolve(&mut self, _env: &mut Env, names: &[String]) -> Vec<(String, Resolution)> {
        self.site.borrow_mut().resolve(names)
    }
    fn before_run(&mut self, env: &mut Env, stack: StackKey, run: usize) {
        let sym = env.intern_symbol("rows");
        assert!(env.output_buffer_for(stack, sym).is_empty(), "the last run's rows were dropped");
        self.before.push(run);
    }
    fn after_run(&mut self, env: &mut Env, stack: StackKey, report: &RunReport) {
        let sym = env.intern_symbol("rows");
        let rows = env.output_buffer_for(stack, sym).to_vec();
        let rows = Json::Array(rows.iter().map(|v| value_to_json(v, env.heap()).unwrap()).collect());
        // Draining print output here is the host's choice: the settled
        // run's is then in `seen` rather than in `env`.
        let printed = env.take_output();
        self.seen.push((report.speculative, printed, rows));
    }
}

#[test]
fn after_run_sees_each_runs_outputs_before_they_are_dropped() {
    let script = "let p = fetch(\"/a\")\nprint(\"title\", p.title)\npush_output(symbol(\"rows\"), p.title)\n";
    let (mut env, stack, site) = host(&[("/a", json!({"title": "A"}))], script);
    let mut rec = Recorder { site, ..Recorder::default() };
    let report = settle_with(&mut env, stack, 10, &mut rec).unwrap();

    assert!(report.settled);
    assert_eq!(rec.before, [0, 1]);
    assert_eq!(
        rec.seen,
        vec![
            (true, vec!["title ".to_string()], json!([""])),
            (false, vec!["title A".to_string()], json!(["A"])),
        ]
    );
    assert_eq!(drain(&mut env, "rows"), json!(["A"]), "the settled run's buffer stays");
}

/// State a speculative run committed is undone: the settled run starts from
/// the state the loop began with, so a `state` initializer reads the real
/// page, not the stand-in.
#[test]
fn a_speculative_runs_state_is_undone() {
    let script = "state first = fetch(\"/a\").title\nprint(first)\n";
    let (mut env, stack, site) = host(&[("/a", json!({"title": "A"}))], script);
    let report = settle(&mut env, stack, 10, |_, names| site.borrow_mut().resolve(names)).unwrap();
    assert!(report.settled, "{report:?}");
    assert_eq!(report.runs.len(), 2);
    assert_eq!(env.take_output(), ["A"]);
}

/// `is_speculative` turns true at the first placeholder of a run and is
/// reset before the next.
#[test]
fn is_speculative_tracks_the_run_so_far() {
    let script = "let a = fetch(\"/a\")\nlet b = fetch(\"/b\")\n";
    let (mut env, stack, site) =
        host(&[("/a", json!({"title": "A"})), ("/b", json!({"title": "B"}))], script);
    let report = settle(&mut env, stack, 10, |_, names| site.borrow_mut().resolve(names)).unwrap();
    assert!(report.settled);
    assert_eq!(report.runs.len(), 2);
    // Run 0: false at /a (nothing handed out yet), true at /b. Run 1: real.
    assert_eq!(site.borrow().speculative_seen, [false, true, false, false]);
    assert!(!is_speculative());
    assert!(take_placeholders().is_empty());
}

// ── Mixing with the query package ────────────────────────────────────────

/// A native that pushes its own stand-in after `note_placeholder`, and one
/// that answers `push_loading`: both count as asked, and are resolved in the
/// same batch.
#[test]
fn placeholders_and_loading_answers_mix() {
    let script = "\
let raw = get_raw(\"/raw\")
let user = load(\"user\")
let page = fetch(\"/a\")
if is_ready(user) then
  print(raw.n, user.name, page.title)
end
";
    let pages = [
        ("/raw", json!({"n": 1})),
        ("user", json!({"name": "ann"})),
        ("/a", json!({"title": "A"})),
    ];
    let (mut env, stack, site) = host(&pages, script);
    let report = settle(&mut env, stack, 10, |_, names| site.borrow_mut().resolve(names)).unwrap();

    assert!(report.settled, "{report:?}");
    assert_eq!(report.runs.len(), 2);
    let r0 = &report.runs[0];
    assert!(r0.speculative);
    assert_eq!(r0.placeholders, names(&["/raw", "/a"]));
    assert_eq!(r0.loading, names(&["user"]));
    assert_eq!(r0.asked(), names(&["/raw", "/a", "user"]));
    assert_eq!(r0.requested, names(&["/raw", "/a", "user"]));
    assert_eq!(env.take_output(), ["1 ann A"]);
}

/// A run with only a loading answer is speculative too, and a loop whose
/// host leaves it loading ends unsettled, as `run_until_settled` does.
#[test]
fn a_loading_answer_alone_makes_a_run_speculative() {
    let (mut env, stack, _site) = host(&[], "let u = load(\"user\")\nprint(is_loading(u))\n");
    let report = settle(&mut env, stack, 10, |_, _| Vec::new()).unwrap();
    assert!(!report.settled);
    assert!(report.runs.iter().all(|r| r.speculative));
    assert!(report.runs[0].placeholders.is_empty());
    assert_eq!(report.unanswered, names(&["user"]));
}
