//! A batch host's settle loop: `Env::run_until_settled` runs a script, hands
//! the host the resources it asked for and didn't get, and runs again until a
//! run asks for nothing new. The host here is a fake web behind a
//! `ResourceProvider`, answered through `PetalCxt::push_answer`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use petal::env::Env;
use petal::stack::StackKey;
use petal::native_fn::{InputClasses, NativeEffects, PetalCxt};
use petal::resource_provider::{ResourceAnswer, ResourceProvider, Resolution};
use petal::resource_table::hash_key;
use petal::value::{json_to_value, value_to_json};
use serde_json::{Value as Json, json};

/// The fake web: `pages` is what exists, `have` what the host has fetched so
/// far (what its natives answer from), `asked` the URL behind each resource
/// key a native answered loading.
#[derive(Default)]
struct Web {
    pages: HashMap<String, Json>,
    have: HashMap<String, ResourceAnswer<Json>>,
    asked: HashMap<u64, String>,
    fetched: Vec<String>,
}

impl ResourceProvider for Web {
    type Key = str;
    type Value = Json;

    fn answer(&mut self, url: &str) -> ResourceAnswer<Json> {
        self.have.get(url).cloned().unwrap_or(ResourceAnswer::Loading)
    }

    fn invalidate(&mut self, url: &str) {
        self.have.remove(url);
    }
}

impl Web {
    /// The settle loop's resolve step: fetch every key handed over.
    fn resolve(&mut self, keys: &[u64]) -> Vec<(u64, Resolution)> {
        keys.iter()
            .map(|key| {
                let url = self.asked[key].clone();
                self.fetched.push(url.clone());
                let (answer, resolution) = match self.pages.get(&url) {
                    Some(page) => (ResourceAnswer::Ready(page.clone()), Resolution::Resolved),
                    None => {
                        let msg = format!("404 {url}");
                        (ResourceAnswer::Errored(msg.clone()), Resolution::Errored(msg))
                    }
                };
                self.have.insert(url, answer);
                (*key, resolution)
            })
            .collect()
    }
}

fn host(pages: &[(&str, Json)], script: &str) -> (Env, StackKey, Rc<RefCell<Web>>) {
    let web = Rc::new(RefCell::new(Web {
        pages: pages.iter().map(|(u, p)| (u.to_string(), p.clone())).collect(),
        ..Web::default()
    }));
    let mut env = Env::new();
    let w = web.clone();
    env.register_native_boxed(
        "fetch",
        move |cx: &mut PetalCxt| {
            let url = cx.get_string(1)?;
            let key = hash_key(url.as_str());
            cx.note_host_read();
            let answer = {
                let mut web = w.borrow_mut();
                web.asked.insert(key, url.clone());
                web.answer(&url)
            };
            cx.push_answer(key, answer, |cx, page| {
                json_to_value(&page, cx.heap_mut()).expect("page converts")
            });
            Ok(1)
        },
        NativeEffects::reads(InputClasses::HOST_DATA),
    );
    let pid = env.load_program(script).unwrap();
    let stack = env.create_stack(pid).unwrap();
    (env, stack, web)
}

fn key(url: &str) -> u64 {
    hash_key(url)
}

fn drain(env: &mut Env, channel: &str) -> Json {
    let sym = env.intern_symbol(channel);
    let values = env.take_output_buffer(sym);
    Json::Array(values.iter().map(|v| value_to_json(v, env.heap()).unwrap()).collect())
}

const LIST_THEN_DETAILS: &str = "\
import io
native fn fetch(url)
push_output(symbol(\"runs\"), 1)
let urls = fetch(\"/list\")
let again = fetch(\"/list\")
for url in urls do
  let d = fetch(url)
  if is_error(d) then
    push_output(symbol(\"errors\"), error_of(d))
  else
    push_output(symbol(\"rows\"), d.title)
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
/// details, run 2 asks for nothing — settled in three runs, each URL fetched
/// once, and the output is the last run's alone.
#[test]
fn list_then_details_settles_in_three_runs() {
    let (mut env, stack, web) = host(&pages(), LIST_THEN_DETAILS);
    let report = env
        .run_until_settled(stack, 10, |_, keys| web.borrow_mut().resolve(keys))
        .unwrap();

    assert!(report.settled, "{report:?}");
    assert!(report.unanswered.is_empty());
    assert_eq!(report.runs.len(), 3, "{report:?}");
    let runs = &report.runs;
    assert_eq!(runs[0].requested, vec![key("/list")], "two fetches of one URL are one request");
    assert_eq!(runs[0].resolved, vec![key("/list")]);
    assert_eq!(runs[1].requested, vec![key("/a"), key("/b")]);
    assert_eq!(runs[1].resolved, vec![key("/a"), key("/b")]);
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
    let report = env
        .run_until_settled(stack, 2, |_, keys| web.borrow_mut().resolve(keys))
        .unwrap();

    assert!(!report.settled);
    assert_eq!(report.runs.len(), 2);
    assert_eq!(report.last().requested, vec![key("/a"), key("/b")]);
    assert!(report.last().resolved.is_empty());
    assert_eq!(report.unanswered, vec![key("/a"), key("/b")]);
    assert_eq!(web.borrow().fetched, vec!["/list"]);
    assert_eq!(drain(&mut env, "rows"), json!([]));
}

/// A fetch that fails: reported as errored, answered as an errored pending
/// value in the next run, and not asked for again — the loop still settles.
#[test]
fn an_errored_resource_settles_as_errored() {
    let mut pages = pages();
    pages.retain(|(u, _)| *u != "/b");
    let (mut env, stack, web) = host(&pages, LIST_THEN_DETAILS);
    let report = env
        .run_until_settled(stack, 10, |_, keys| web.borrow_mut().resolve(keys))
        .unwrap();

    assert!(report.settled, "{report:?}");
    assert_eq!(report.runs.len(), 3, "{report:?}");
    assert_eq!(report.runs[1].resolved, vec![key("/a")]);
    assert_eq!(report.runs[1].errored, vec![(key("/b"), "404 /b".to_string())]);
    assert_eq!(drain(&mut env, "rows"), json!(["A"]));
    assert_eq!(drain(&mut env, "errors"), json!(["404 /b"]));
}

/// A host that leaves a key loading (out of budget, say): the next run asks
/// for it again, which is nothing new, so the loop stops there instead of
/// spinning to its limit.
#[test]
fn a_key_the_host_leaves_loading_stops_the_loop() {
    let (mut env, stack, _web) = host(&pages(), LIST_THEN_DETAILS);
    let report = env.run_until_settled(stack, 10, |_, _| Vec::new()).unwrap();

    assert!(!report.settled);
    assert_eq!(report.runs.len(), 2);
    assert_eq!(report.runs[0].requested, vec![key("/list")]);
    assert!(report.runs[1].requested.is_empty());
    assert_eq!(report.unanswered, vec![key("/list")]);
}

/// The resolve step sees the `Env`: a host can keep its answers in the
/// resource table itself, which the test-only `__pending` reads.
#[test]
fn a_host_can_answer_through_the_resource_table() {
    let mut env = Env::new();
    let pid = env.load_program("let x = __pending(\"x\")\nx + 1\n").unwrap();
    let stack = env.create_stack(pid).unwrap();
    let report = env
        .run_until_settled(stack, 5, |env, keys| {
            let frame = env.frame(stack);
            for k in keys {
                env.resources_mut(stack).unwrap().resolve(*k, petal::value::Value::Int(41), frame);
            }
            keys.iter().map(|k| (*k, Resolution::Resolved)).collect()
        })
        .unwrap();
    assert!(report.settled, "{report:?}");
    assert_eq!(report.runs.len(), 2);
    assert_eq!(report.runs[0].requested, vec![hash_key("x")]);
    assert_eq!(report.value, Some(petal::value::Value::Int(42)));
}

/// `unanswered_requests` lists what the current frame asked for and got
/// only a loading answer for; a key resolved by the host isn't a request.
#[test]
fn unanswered_requests_is_the_frames_loading_asks() {
    let mut env = Env::new();
    let pid = env
        .load_program("let a = __pending(\"a\")\nlet a2 = __pending(\"a\")\nlet b = __pending(\"b\")\n0\n")
        .unwrap();
    let stack = env.create_stack(pid).unwrap();
    env.run(stack).unwrap();
    assert_eq!(env.unanswered_requests(stack), vec![hash_key("a"), hash_key("b")]);

    // Next frame: `b` landed; `a` is asked for again and still loading.
    let frame = env.frame(stack) + 1;
    env.resources_mut(stack).unwrap().resolve(hash_key("b"), petal::value::Value::Int(1), frame);
    env.reset_stack(stack).unwrap();
    env.advance_frame(stack);
    env.run(stack).unwrap();
    assert_eq!(env.unanswered_requests(stack), vec![hash_key("a")]);

    // A key the host resolves without the script asking is not a request.
    env.resources_mut(stack).unwrap().resolve(hash_key("c"), petal::value::Value::Int(1), frame);
    assert_eq!(env.unanswered_requests(stack), vec![hash_key("a")]);
}
