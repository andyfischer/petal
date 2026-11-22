//! What each Petal construct becomes in React.

use transpile_to_react_js::{Dialect, Options, Output, transpile};

fn run(src: &str) -> Output {
    run_as(src, None)
}

fn run_as(src: &str, dialect: Option<Dialect>) -> Output {
    let opts = Options { dialect, file_name: "test.ptl".into(), ..Options::default() };
    transpile(src, &opts).unwrap_or_else(|e| panic!("transpile failed: {e}"))
}

fn has_warning(o: &Output, code: &str) -> bool {
    o.warnings.iter().any(|w| w.code == code)
}

#[track_caller]
fn contains(o: &Output, needle: &str) {
    assert!(o.code.contains(needle), "expected `{needle}` in:\n{}", o.code);
}

#[test]
fn state_is_use_state() {
    let o = run(r#"
fn counter(label)
  state count = 0
  <div class="counter">{label}: {count}</div>

counter("Clicks")
"#);
    contains(&o, r#"import { useState } from "react";"#);
    contains(&o, "export function Counter({ label }) {");
    contains(&o, "const [count, setCount] = useState(0);");
    contains(&o, r#"<div className="counter">{label}: {count}</div>"#);
    contains(&o, "export default function Test() {");
    contains(&o, r#"return <Counter label="Clicks" />;"#);
}

#[test]
fn an_expensive_initializer_is_lazy() {
    let o = run("fn f()\n  state xs = range(0, 100)\n  <div>{len(xs)}</div>\nf()\n");
    contains(&o, "useState(() => range(0, 100))");
}

#[test]
fn click_polls_become_handlers_that_call_the_setter() {
    let o = run(r#"
import web

fn app()
  let toggle_id = next_id()
  let reset_id = next_id()
  state open = false
  state count = 0
  if clicked(toggle_id) then
    open = !open
    count = count + 1
  if clicked(reset_id) then
    count = 0
  <div>
    <button eid={toggle_id}>{if open then "Close" else "Open"}</button>
    <button eid={reset_id}>Reset {count}</button>
  </div>

app()
"#);
    assert_eq!(o.dialect, Dialect::Web);
    // State first, then the handlers that write it.
    let state = o.code.find("const [count, setCount]").unwrap();
    let handler = o.code.find("const toggleId = () => {").unwrap();
    assert!(state < handler, "{}", o.code);
    // Two writes: a copy each, the setters at the end.
    contains(&o, "let nextOpen = open;");
    contains(&o, "nextCount = nextCount + 1;");
    contains(&o, "setOpen(nextOpen);");
    contains(&o, "const resetId = () => setCount(0);");
    contains(&o, "<button onClick={toggleId}>");
    contains(&o, "export default App;");
    assert!(o.warnings.is_empty(), "{:?}", o.warnings);
}

#[test]
fn a_function_returning_an_element_is_a_component_and_map_keys_it() {
    let o = run(r#"
let rows = [{id: "a", title: "One"}, {id: "b", title: "Two"}]
fn row(r)
  <li>{r.title}</li>
let list = <ul>{map(rows, row)}</ul>
list
"#);
    contains(&o, "export function Row({ r }) {");
    contains(&o, "rows.map((r, i) => <Row key={i} r={r} />)");
}

#[test]
fn a_keyed_for_loop_keys_by_its_key() {
    let o = run(r#"
let tasks = [{id: 7, title: "write"}]
fn view()
  <ul>{for t in tasks key t.id do <li>{t.title}</li> end}</ul>
view()
"#);
    contains(&o, "tasks.map((t) => <li key={t.id}>{t.title}</li>)");
}

#[test]
fn state_in_a_loop_is_reported() {
    let o = run("fn f(xs)\n  for x in xs do\n    state n = 0\n  <div />\nf([1])\n");
    assert!(has_warning(&o, "state-in-loop"), "{:?}", o.warnings);
}

#[test]
fn a_stateful_function_returning_a_value_is_a_hook() {
    let o = run(r#"
fn stable_random()
  state val = random_int(100)
  val

fn view()
  let n = stable_random()
  <div>{n}</div>
view()
"#);
    contains(&o, "export function useStableRandom() {");
    contains(&o, "const n = useStableRandom();");
}

#[test]
fn a_top_level_state_var_is_a_shared_cell() {
    let o = run(r#"
state var theme = "dark"
fn label()
  <span>{get theme}</span>
label()
"#);
    contains(&o, r#"export const themeCell = cell("dark");"#);
    contains(&o, "const theme = useCell(themeCell);");
}

#[test]
fn writing_state_while_rendering_is_reported() {
    let o = run("state ticks = 0\nticks += 1\n<div>{ticks}</div>\n");
    assert!(has_warning(&o, "state-write-in-render"), "{:?}", o.warnings);
    contains(&o, "let ticks = ticksState;");
}

#[test]
fn records_are_values() {
    let o = run("fn f(r)\n  r.a.b = 3\n  r\nprint(f({a: {b: 1}}))\n");
    contains(&o, "r = { ...r, a: { ...r.a, b: 3 } };");
}

#[test]
fn equality_and_nil() {
    let o = run("fn f(a, b)\n  if a == nil then 1 else if a == b then 2 else if a == \"x\" then 3 else 4 end\nprint(f(1, 2))\n");
    contains(&o, "a == null");
    contains(&o, "eq(a, b)");
    contains(&o, r#"a === "x""#);
}

#[test]
fn color_literals_and_interpolation() {
    let o = run("let c = :FF9500\nlet s = \"{c.r} of `{1}`\"\nprint(s)\n");
    contains(&o, r#"hex("FF9500")"#);
    contains(&o, "`${c.r} of \\`${1}\\``");
}

#[test]
fn named_arguments_go_to_their_parameters() {
    let o = run("fn area(w, h = 1, scale = 1) w * h * scale\nprint(area(2, scale: 3))\n");
    contains(&o, "area(2, undefined, 3)");
}

#[test]
fn overloads_are_one_function_per_arity() {
    let o = run("fn area(r) r * r\nfn area(w, h) w * h\nprint(area(2), area(2, 3))\n");
    contains(&o, "function area_1(r)");
    contains(&o, "function area_2(w, h)");
    contains(&o, "console.log(area_1(2), area_2(2, 3))");
    contains(&o, "export function area(...args) {");
}

#[test]
fn a_collecting_loop_that_skips_is_a_loop() {
    let o = run("fn evens(xs)\n  for x in xs do\n    if x % 2 == 1 then continue\n    x * 10\nprint(evens([1, 2]))\n");
    contains(&o, "const collected = [];");
    contains(&o, "continue;");
    contains(&o, "collected.push(x * 10);");
    contains(&o, "return collected;");
}

#[test]
fn match_is_an_if_chain() {
    let o = run(r#"
enum Shape
  circle(r),
  square(side)

fn area(s)
  match s
    when Shape.circle(r) -> 3 * r * r
    when Shape.square(a) -> a * a

print(area(Shape.circle(2)))
"#);
    contains(&o, "circle: (r) => ({ tag: \"circle\", r }),");
    contains(&o, "if (matched?.tag === \"circle\") {");
    contains(&o, "const r = matched.r;");
}

#[test]
fn immediate_mode_drawing_is_reported() {
    let o = run("import ui\ndraw_rect(0, 0, 10, 10, :FF0000)\n");
    assert_eq!(o.dialect, Dialect::Immediate);
    assert!(has_warning(&o, "immediate-mode"));
    assert!(has_warning(&o, "unknown-name"));
    contains(&o, "const drawRect = unconverted(\"draw_rect\");");
}

#[test]
fn an_unknown_tag_is_imported_from_the_elements_module() {
    let o = run_as("let x = <card title=\"Hi\" />\nx\n", Some(Dialect::Generic));
    contains(&o, r#"import { Card } from "./elements.jsx";"#);
    assert_eq!(o.elements, vec!["Card".to_string()]);
    assert!(has_warning(&o, "unknown-element"));
}

#[test]
fn a_runtime_name_the_program_binds_is_aliased() {
    let o = run("let cell = 3\nstate var x = 1\nprint(cell, get x)\n");
    contains(&o, "cell as petalCell");
    contains(&o, "export const xCell = petalCell(1);");
}

const DESIGN: &str = r#"
import design

let tok = tokens({color: {ink: modes(light: :0A0A0E, dark: :F2F2F7), card: :FFFFFF}})

fn card(p, cx)
  <vstack padding={16} fill={tok.color.card}>
    <text id="title" color={tok.color.ink}>{p.title}</text>
    <slot />
    <slot name="footer"><text>No footer</text></slot>
  </vstack>

let components = [component("card", card, group: "Cards", props: {title: "Card"})]

let pages = [
  <page id="home" title="Home" layout="canvas">
    <frame id="home_ios" device="iphone_16">
      <vstack width="fill" gap={12}>
        <instance of="card" title="Today">
          <text>Body</text>
          <text slot="footer">Footer</text>
        </instance>
        {map(range(3), fn(i) -> <instance of="card" title={"Run {i}"} />)}
      </vstack>
    </frame>
  </page>,
]

design({name: "Test", tokens: tok}, components, pages)
"#;

#[test]
fn a_design_component_merges_its_props_over_its_defaults() {
    let o = run(DESIGN);
    assert_eq!(o.dialect, Dialect::Design);
    contains(&o, "export function Card(props) {");
    contains(&o, "const p = component_props(props, Card.defaults);");
    contains(&o, "const cx = useCx();");
    contains(&o, "return instance_root(props, (");
    contains(&o, "Card.defaults = { title: \"Card\" };");
    contains(&o, r#"component("card", Card, { group: "Cards", props: Card.defaults })"#);
    contains(&o, "{slot(props.children, null, null)}");
    contains(&o, r#"{slot(props.children, "footer", <Text>No footer</Text>)}"#);
    contains(&o, "modes(hex(\"0A0A0E\"), { dark: hex(\"F2F2F7\") })");
}

#[test]
fn instances_are_jsx_and_frames_are_components() {
    let o = run(DESIGN);
    contains(&o, "<Card title=\"Today\">");
    contains(&o, "<Text slot=\"footer\">Footer</Text>");
    contains(&o, "range(3).map((i) => <Card key={i} title={`Run ${i}`} />)");
    contains(&o, "export function HomeIos() {");
    contains(&o, "HomeIos.frameId = \"home_ios\";");
    contains(&o, "<HomeIos />");
    contains(&o, "export default function TestDesign() {");
    contains(&o, "return design({ name: \"Test\", tokens: tok }, components, pages);");
    assert_eq!(o.frames, vec![("home_ios".to_string(), "HomeIos".to_string())]);
    assert!(o.warnings.is_empty(), "{:?}", o.warnings);
}

#[test]
fn comments_are_kept() {
    let o = run("# The rows.\nlet rows = [1, 2]\nprint(rows)\n");
    contains(&o, "// The rows.\nexport const rows = [1, 2];");
}

#[test]
fn warnings_are_also_todo_comments_in_the_output() {
    let o = run("print(mystery(1))\n");
    contains(&o, "// TODO(petal-to-react) [unknown-name]: `mystery`");
}

/// Every runtime export the transpiler can name is in the runtime files,
/// and they parse as the transpiler reads them.
#[test]
fn runtime_exports_are_read() {
    let rt = transpile_to_react_js::sigs::exports(transpile_to_react_js::RUNTIME);
    for name in ["map", "len", "range", "eq", "hex", "cell", "unconverted", "format", "with_at", "at"] {
        assert!(rt.contains_key(name), "petal-runtime.js does not export {name}");
    }
    let d = transpile_to_react_js::sigs::exports(transpile_to_react_js::DESIGN_RUNTIME);
    for name in ["VStack", "Text", "Frame", "tokens", "modes", "per", "component", "component_props", "instance_root", "slot", "useCx", "DEVICES", "IOS_TEXT_STYLES"] {
        assert!(d.contains_key(name), "petal-design.jsx does not export {name}");
    }
    assert_eq!(d["modes"].positional, ["light"]);
    assert_eq!(d["modes"].options, ["dark", "light_hc", "dark_hc"]);
}

#[test]
fn only_pub_declarations_are_exported_from_a_module_that_uses_pub() {
    let o = run("let M = {gap: 4}\nfn helper(x) x * 2\npub fn twice(x) helper(x) + M.gap\npub let LIMIT = 3\n");
    contains(&o, "\nconst M = { gap: 4 };");
    contains(&o, "\nfunction helper(x) {");
    contains(&o, "export function twice(x) {");
    contains(&o, "export const LIMIT = 3;");
    // With no `pub` anywhere (an entry file, a design), everything is.
    let o = run("let rows = [1]\nfn f(x) x\nprint(f(rows))\n");
    contains(&o, "export const rows = [1];");
    contains(&o, "export function f(x) {");
}

#[test]
fn a_tap_sets_a_state_var_and_what_reads_it_subscribes() {
    let o = run(r#"
import design

state var tab = "a"

let pages = [
  <page id="p" title="P" layout="canvas">
    <frame id="f" device="iphone_16">
      <vstack>
        <text>{get tab}</text>
        {for t in ["a", "b"] key t do
          <text on_tap={fn()
            set tab = t
          end}>{if get tab == t then "*" else t end}</text>
        end}
        <text on_tap={fn()
          set tab = "a"
          print(get tab)
        end}>Reset</text>
      </vstack>
    </frame>
  </page>,
]
design({name: "T"}, [], pages)
"#);
    contains(&o, r#"export const tabCell = cell("a");"#);
    // The lifted frame subscribes, and reads the subscribed value, in its
    // loops too.
    contains(&o, "export function F() {\n  const tab = useCell(tabCell);");
    contains(&o, "<Text>{tab}</Text>");
    contains(&o, r#"eq(tab, t) ? "*" : t"#);
    // The tap writes the cell; a callback that writes it reads it fresh.
    contains(&o, "tabCell.set(t);");
    contains(&o, "console.log(tabCell.get());");
}
