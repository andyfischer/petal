//! A tiny test vocabulary: a `shapes` package whose `doc(items)` hands the
//! document to the host through the `doc_emit` native, with a helper that
//! builds an element inside the package.

#![allow(dead_code)]

use petal::env::Env;
use petal::native_fn::NativeEffects;
use programs_as_documents::{DocumentRun, OutputKey, OutputNode, OutputSource, Runner};

pub const SHAPES: &str = "native fn doc_emit(items)

pub fn doc(items)
  doc_emit(items)
end

pub fn badge(label, x)
  <badge label={label} x={x} size={12} />
end

pub let LIBRARY = [
  <libitem id=\"lib-a\" />,
  <libitem id=\"lib-b\" />,
]
";

pub const MANIFEST: &str = "[package]\nname = \"shapes\"\nversion = \"0.1.0\"\n";

pub fn setup(env: &mut Env) -> Result<(), String> {
    env.register_native_boxed(
        "doc_emit",
        |cx| {
            let sym = cx.intern_symbol("doc_emit");
            for i in 1..=cx.arg_count() {
                let v = cx.get_value(i)?;
                cx.push_output(sym, v);
            }
            cx.push_nil();
            Ok(1)
        },
        NativeEffects::EMITS,
    );
    let files = [("shapes", SHAPES), (petal::package::MANIFEST_FILE, MANIFEST)];
    env.register_package("shapes", files).map_err(|e| e.to_string())?;
    env.provides_package("shapes")
}

/// Documents that end in `doc(items)`.
pub fn emit_runner() -> Runner {
    Runner::new("doc.ptl", setup).with_output(OutputSource::Buffer { symbol: "doc_emit".into(), last: Some(1) })
}

/// Documents whose value is the result.
pub fn value_runner() -> Runner {
    Runner::new("doc.ptl", setup)
}

pub fn run(runner: &Runner, src: &str) -> DocumentRun {
    runner.run(src).unwrap_or_else(|e| panic!("run failed: {e}\n{src}"))
}

/// The nodes with tag `tag`, in result order.
pub fn tagged<'a>(run: &'a DocumentRun, tag: &str) -> Vec<&'a OutputNode> {
    run.map.nodes.iter().filter(|n| n.tag == tag).collect()
}

pub fn key_of(run: &DocumentRun, tag: &str, i: usize) -> OutputKey {
    tagged(run, tag)[i].key.clone()
}
