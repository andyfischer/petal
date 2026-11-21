//! The `query` package end to end: a host answering through a `QueryHost`,
//! the readers of a pending answer, the error a script gets for using one as
//! data, and the batch host's settle loop.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use petal::env::Env;
use petal::native_fn::PetalCxt;
use petal::stack::StackKey;
use petal::value::{Value, json_to_value, value_to_json};
use petal_stdlib_query::{QueryHost, Resolution, ResourceAnswer, run_until_settled, swap_host};
use serde_json::{Value as Json, json};

/// A fake web: `pages` is what exists, `have` what the host has fetched so
/// far (what `query` answers from).
#[derive(Default)]
struct Web {
    pages: HashMap<String, Json>,
    have: HashMap<String, ResourceAnswer<Json>>,
    fetched: Vec<String>,
}

/// The `QueryHost` the thread holds: a shared view of the test's `Web`.
struct WebHost(Rc<RefCell<Web>>);

impl QueryHost for WebHost {
    fn answer(&mut self, cx: &mut PetalCxt, kind: &str, arg: Value) -> Result<ResourceAnswer<Value>, String> {
        assert_eq!(kind, "page");
        let url = match arg {
            Value::String(id) => cx.heap().get_string(id).to_string(),
            other => return Err(format!("page wants a url, got {}", other.type_name())),
        };
        let answer = self.0.borrow().have.get(&url).cloned().unwrap_or(ResourceAnswer::Loading);
        Ok(answer.map(|page| json_to_value(&page, cx.heap_mut()).expect("page converts")))
    }

    fn invalidate(&mut self, cx: &mut PetalCxt, _kind: &str, arg: Value) -> Result<(), String> {
        if let Value::String(id) = arg {
            let url = cx.heap().get_string(id).to_string();
            self.0.borrow_mut().have.remove(&url);
        }
        Ok(())
    }
}

impl Web {
    /// The settle loop's resolve step: fetch every name handed over.
    fn resolve(&mut self, names: &[String]) -> Vec<(String, Resolution)> {
        names
            .iter()
            .map(|name| {
                // `query("page", "/a")` → `/a`.
                let url: String = serde_json::from_str(
                    name.trim_start_matches("query(\"page\", ").trim_end_matches(')'),
                )
                .expect("a query name");
                self.fetched.push(url.clone());
                let (answer, resolution) = match self.pages.get(&url) {
                    Some(page) => (ResourceAnswer::Ready(page.clone()), Resolution::Resolved),
                    None => {
                        let msg = format!("404 {url}");
                        (ResourceAnswer::Errored(msg.clone()), Resolution::Errored(msg))
                    }
                };
                self.have.insert(url, answer);
                (name.clone(), resolution)
            })
            .collect()
    }
}

fn host(pages: &[(&str, Json)], script: &str) -> (Env, StackKey, Rc<RefCell<Web>>) {
    let web = Rc::new(RefCell::new(Web {
        pages: pages.iter().map(|(u, p)| (u.to_string(), p.clone())).collect(),
        ..Web::default()
    }));
    swap_host(Some(Box::new(WebHost(web.clone()))));
    let mut env = Env::new();
    petal_stdlib_query::register(&mut env);
    env.set_echo(false);
    let pid = env.load_program(script).unwrap_or_else(|e| panic!("{e}"));
    let stack = env.create_stack(pid).unwrap();
    (env, stack, web)
}

/// Run `script` once with no host installed: every query loading.
fn run_alone(script: &str) -> (Result<Vec<String>, String>, Env) {
    swap_host(None);
    let mut env = Env::new();
    petal_stdlib_query::register(&mut env);
    env.set_echo(false);
    let pid = match env.load_program(script) {
        Ok(pid) => pid,
        Err(e) => return (Err(e.to_string()), env),
    };
    let stack = env.create_stack(pid).unwrap();
    let r = env.run(stack).map(|_| ()).map_err(|e| e.to_string());
    let out = env.take_output();
    (r.map(|_| out), env)
}

fn name(url: &str) -> String {
    format!("query(\"page\", {url:?})")
}

fn drain(env: &mut Env, channel: &str) -> Json {
    let sym = env.intern_symbol(channel);
    let values = env.take_output_buffer(sym);
    Json::Array(values.iter().map(|v| value_to_json(v, env.heap()).unwrap()).collect())
}

// ── The readers ──────────────────────────────────────────────────────────

#[test]
fn a_query_with_no_host_is_loading() {
    let (out, _) = run_alone(
        "import query\n\
         let u = query(\"user\", 7)\n\
         print(is_ready(u), is_loading(u), is_error(u), error_of(u))\n\
         print(is_ready(1), is_ready(nil), is_loading(\"x\"), error_of([]))\n",
    );
    assert_eq!(out.unwrap(), ["false true false nil", "true true false nil"]);
}

#[test]
fn a_failed_answer_carries_its_message() {
    let web = Rc::new(RefCell::new(Web::default()));
    web.borrow_mut().have.insert("/x".into(), ResourceAnswer::Errored("404".into()));
    let (mut env, stack, _web) = host(&[], "import query\nlet p = query(\"page\", \"/x\")\nprint(is_error(p), is_loading(p), error_of(p))\n");
    swap_host(Some(Box::new(WebHost(web))));
    env.run(stack).unwrap();
    assert_eq!(env.take_output(), ["true false 404"]);
}

#[test]
fn a_ready_answer_is_the_value_itself() {
    let (mut env, stack, web) = host(&[], "import query\nlet p = query(\"page\", \"/a\")\nprint(is_ready(p), p.title)\n");
    web.borrow_mut().have.insert("/a".into(), ResourceAnswer::Ready(json!({"title": "A"})));
    env.run(stack).unwrap();
    assert_eq!(env.take_output(), ["true A"]);
}

#[test]
fn invalidate_reaches_the_host() {
    let (mut env, stack, web) = host(
        &[],
        "import query\nprint(is_ready(query(\"page\", \"/a\")))\ninvalidate(\"page\", \"/a\")\nprint(is_ready(query(\"page\", \"/a\")))\n",
    );
    web.borrow_mut().have.insert("/a".into(), ResourceAnswer::Ready(json!(1)));
    env.run(stack).unwrap();
    assert_eq!(env.take_output(), ["true", "false"]);
}

// ── Using a pending answer as data is an error ───────────────────────────

fn error_of_using(expr: &str) -> String {
    let script = format!("import query\nlet u = query(\"user\", 7)\n{expr}\n");
    match run_alone(&script).0 {
        Ok(out) => panic!("`{expr}` ran: {out:?}"),
        Err(e) => e,
    }
}

#[test]
fn using_a_pending_answer_as_data_fails_loudly() {
    for expr in ["u + 1", "u.name", "u[0]", "u < 2", "for x in u do print(x) end", "len(u)"] {
        let e = error_of_using(expr);
        assert!(e.contains("handle") || e.contains("pending answer"), "`{expr}`: {e}");
    }
}

#[test]
fn calling_a_method_on_a_pending_answer_names_the_query() {
    let e = error_of_using("u.greet()");
    assert!(e.contains("query(\"user\", 7), still loading"), "{e}");
    assert!(e.contains("is_ready"), "{e}");
}

// ── The settle loop ──────────────────────────────────────────────────────

const LIST_THEN_DETAILS: &str = "\
import io
import query
push_output(symbol(\"runs\"), 1)
let urls = query(\"page\", \"/list\")
let again = query(\"page\", \"/list\")
if is_ready(urls) then
  for url in urls do
    let d = query(\"page\", url)
    if is_error(d) then
      push_output(symbol(\"errors\"), error_of(d))
    else if is_ready(d) then
      push_output(symbol(\"rows\"), d.title)
    end
  end
end
\"done\"
";

fn pages() -> Vec<(&'static str, Json)> {
    vec![
        ("/list", json!(["/a", "/b"])),
        ("/a", json!({"title": "A"})),
        ("/b", json!({"title": "B"})),
    ]
}

/// A list page then its detail pages: run 0 asks for the list, run 1 for the
/// details, run 2 asks for nothing: settled in three runs, each URL fetched
/// once, and the output is the last run's alone.
#[test]
fn list_then_details_settles_in_three_runs() {
    let (mut env, stack, web) = host(&pages(), LIST_THEN_DETAILS);
    let report = run_until_settled(&mut env, stack, 10, |_, names| web.borrow_mut().resolve(names)).unwrap();

    assert!(report.settled, "{report:?}");
    assert!(report.unanswered.is_empty());
    assert_eq!(report.runs.len(), 3, "{report:?}");
    let runs = &report.runs;
    assert_eq!(runs[0].requested, vec![name("/list")], "two asks of one URL are one request");
    assert_eq!(runs[0].resolved, vec![name("/list")]);
    assert_eq!(runs[1].requested, vec![name("/a"), name("/b")]);
    assert_eq!(runs[1].resolved, vec![name("/a"), name("/b")]);
    assert!(runs[2].requested.is_empty() && runs[2].resolved.is_empty());
    assert!(runs.iter().all(|r| r.error.is_none() && r.errored.is_empty()));
    assert_eq!(web.borrow().fetched, vec!["/list", "/a", "/b"]);

    let value = report.value.expect("the last run's value");
    assert_eq!(value_to_json(&value, env.heap()).unwrap(), json!("done"));
    assert_eq!(drain(&mut env, "rows"), json!(["A", "B"]));
    assert_eq!(drain(&mut env, "runs"), json!([1]), "earlier runs' output is dropped");
}

/// A run limit that comes first: the last run's requests are reported but
/// not handed to the host, and the report says the loop didn't settle.
#[test]
fn max_runs_stops_the_loop_unsettled() {
    let (mut env, stack, web) = host(&pages(), LIST_THEN_DETAILS);
    let report = run_until_settled(&mut env, stack, 2, |_, names| web.borrow_mut().resolve(names)).unwrap();

    assert!(!report.settled);
    assert_eq!(report.runs.len(), 2);
    assert_eq!(report.last().requested, vec![name("/a"), name("/b")]);
    assert!(report.last().resolved.is_empty());
    assert_eq!(report.unanswered, vec![name("/a"), name("/b")]);
    assert_eq!(web.borrow().fetched, vec!["/list"]);
    assert_eq!(drain(&mut env, "rows"), json!([]));
}

/// A fetch that fails: reported as errored, answered as a failed pending
/// answer in the next run, and not asked for again: the loop still settles.
#[test]
fn a_failed_fetch_settles_as_errored() {
    let mut pages = pages();
    pages.retain(|(u, _)| *u != "/b");
    let (mut env, stack, web) = host(&pages, LIST_THEN_DETAILS);
    let report = run_until_settled(&mut env, stack, 10, |_, names| web.borrow_mut().resolve(names)).unwrap();

    assert!(report.settled, "{report:?}");
    assert_eq!(report.runs.len(), 3, "{report:?}");
    assert_eq!(report.runs[1].resolved, vec![name("/a")]);
    assert_eq!(report.runs[1].errored, vec![(name("/b"), "404 /b".to_string())]);
    assert_eq!(drain(&mut env, "rows"), json!(["A"]));
    assert_eq!(drain(&mut env, "errors"), json!(["404 /b"]));
}

/// A host that leaves a name loading: the next run asks for it again, which
/// is nothing new, so the loop stops there instead of spinning to its limit.
#[test]
fn a_name_the_host_leaves_loading_stops_the_loop() {
    let (mut env, stack, _web) = host(&pages(), LIST_THEN_DETAILS);
    let report = run_until_settled(&mut env, stack, 10, |_, _| Vec::new()).unwrap();

    assert!(!report.settled);
    assert_eq!(report.runs.len(), 2);
    assert_eq!(report.runs[0].requested, vec![name("/list")]);
    assert!(report.runs[1].requested.is_empty());
    assert_eq!(report.unanswered, vec![name("/list")]);
}

/// A script that uses an answer before testing it fails while the answer is
/// loading; the loop answers what it asked for and the retry succeeds.
#[test]
fn a_run_that_fails_on_a_loading_answer_is_retried() {
    let script = "import query\nlet urls = query(\"page\", \"/list\")\nlen(urls)\n";
    let (mut env, stack, web) = host(&pages(), script);
    let report = run_until_settled(&mut env, stack, 10, |_, names| web.borrow_mut().resolve(names)).unwrap();

    assert!(report.settled, "{report:?}");
    assert_eq!(report.runs.len(), 2);
    assert!(report.runs[0].error.is_some(), "{report:?}");
    assert_eq!(report.value, Some(Value::Int(2)));
}
