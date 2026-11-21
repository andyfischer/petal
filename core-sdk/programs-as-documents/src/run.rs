//! Running a document: with element tracing on, to edit it, or plainly, to
//! read its output.
//!
//! A host describes how to run its documents once, as a [`Runner`]: the path
//! the document is loaded as (relative imports resolve from it), a setup
//! closure that registers the host's packages and natives, and where the
//! document's output is read from ([`OutputSource`]). Usually that is an
//! [`OutputCall`]: the call a document ends in to hand its output to the
//! host (`canvas(doc, layers)`), which the runner registers itself.
//!
//! [`Runner::run`] runs a source text in a fresh [`Env`] with element-origin
//! tracing on and returns a [`DocumentRun`]: the output values and the
//! [`OutputMap`] that ties each element in them back to the source.
//! [`Runner::evaluate`] runs it without tracing, for a host that only reads
//! the output (an export, a command line), and returns it as plain JSON
//! ([`Evaluated`]).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use petal::env::Env;
use petal::native_fn::NativeEffects;
use petal::program::{Program, ProgramId};
use petal::stack::StackKey;
use petal::static_value::StaticValue;
use petal::value::Value;

use crate::output_map::{OutputKey, OutputMap, OutputNode};

/// The call a document ends in to hand its output to the host:
/// `canvas(doc, layers)`, a function of the host's interface package that
/// passes its values to the native `native` (`native fn canvas_output(doc,
/// layers)`). The [`Runner`] registers that native; its arguments are the
/// document's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputCall {
    /// The native the call ends in. Its arguments go to the output buffer of
    /// the same name.
    pub native: &'static str,
    /// The call as a document writes it, for the error when it doesn't:
    /// `"canvas(doc, layers)"`.
    pub call: &'static str,
    /// How many values the native is passed.
    pub arity: usize,
}

/// Where a run's output is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputSource {
    /// The value the script evaluates to (its last expression).
    ReturnValue,
    /// The arguments of the last call to an [`OutputCall`]'s native. A run
    /// that never made the call is an error naming it ("the script never
    /// called canvas(doc, layers)").
    Call(OutputCall),
    /// [`Call`](Self::Call) for a host that opens documents of several kinds:
    /// the output of whichever of these calls the script made, the first
    /// listed when it made more than one. The run says which
    /// ([`DocumentRun::output_call`]).
    AnyCall(Vec<OutputCall>),
    /// The values pushed to the output buffer of symbol `symbol` by a native
    /// the host registers itself. With `last: Some(n)`, only the last `n`
    /// values; a run that pushed fewer is an error.
    Buffer { symbol: String, last: Option<usize> },
}

impl OutputSource {
    /// The output calls whose natives the runner registers.
    fn calls(&self) -> &[OutputCall] {
        match self {
            OutputSource::Call(c) => std::slice::from_ref(c),
            OutputSource::AnyCall(cs) => cs,
            _ => &[],
        }
    }
}

/// Register `native` as a native that pushes its arguments to the output
/// buffer of its own name and returns `nil`: what an [`OutputCall`] ends in.
pub fn register_output_native(env: &mut Env, native: &'static str) {
    env.register_native_boxed(
        native,
        move |cx| {
            let sym = cx.intern_symbol(native);
            for i in 1..=cx.arg_count() {
                let v = cx.get_value(i)?;
                cx.push_output(sym, v);
            }
            cx.push_nil();
            Ok(1)
        },
        NativeEffects::EMITS,
    );
}

/// A host's setup for an [`Env`] before a document is loaded into it.
pub type Setup = dyn Fn(&mut Env) -> Result<(), String>;

/// How a host runs its documents. Cheap to clone.
#[derive(Clone)]
pub struct Runner {
    path: PathBuf,
    setup: Rc<Setup>,
    output: OutputSource,
    derived: Vec<usize>,
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("path", &self.path)
            .field("output", &self.output)
            .field("derived", &self.derived)
            .finish()
    }
}

impl Runner {
    /// Run documents loaded as `path`, reading the script's return value.
    /// `setup` registers the host's packages and natives in each fresh env.
    pub fn new(path: impl Into<PathBuf>, setup: impl Fn(&mut Env) -> Result<(), String> + 'static) -> Runner {
        Runner {
            path: path.into(),
            setup: Rc::new(setup),
            output: OutputSource::ReturnValue,
            derived: Vec::new(),
        }
    }

    /// Read the result from `output` instead.
    pub fn with_output(mut self, output: OutputSource) -> Runner {
        self.output = output;
        self
    }

    /// Output values `derived` (indexes into the output) are made from the
    /// others by the host's packages, not written by the document: an
    /// index of the elements the others hold, keyed by where they are
    /// (petal-design's instance expansions, keyed by each instance's path).
    /// Their elements are in the output map like any others, but an edit's
    /// rerun does not compare them: a change there is what the edit means
    /// for the host's packages, not another output the edit changes.
    pub fn with_derived(mut self, derived: Vec<usize>) -> Runner {
        self.derived = derived;
        self
    }

    /// The result values that are derived (see [`with_derived`](Self::with_derived)).
    pub fn derived(&self) -> &[usize] {
        &self.derived
    }

    /// The path documents are loaded as.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn output(&self) -> &OutputSource {
        &self.output
    }

    /// A fresh env with the host's setup and element tracing on, `source`
    /// loaded into it, and a stack ready to run it (not yet run): what a run
    /// starts from, and the checkpoint a direct-manipulation solve re-runs.
    pub fn prepare(&self, source: &str) -> Result<(Env, ProgramId, StackKey), String> {
        self.prepare_traced(source, true)
    }

    fn prepare_traced(&self, source: &str, trace: bool) -> Result<(Env, ProgramId, StackKey), String> {
        let mut env = Env::new();
        (self.setup)(&mut env)?;
        for c in self.output.calls() {
            register_output_native(&mut env, c.native);
        }
        env.enable_element_trace(trace);
        let program = env.load_program_at(source, &self.path)?;
        let stack = env.create_stack(program)?;
        Ok((env, program, stack))
    }

    /// Run `source` in a fresh env with element tracing on, and map its
    /// output. Errors are a load (parse, compile) or run failure, or a
    /// missing output call, as a message for the user.
    pub fn run(&self, source: &str) -> Result<DocumentRun, String> {
        let (mut env, program, stack) = self.prepare(source)?;
        let result = env.run(stack)?;
        let log = env.take_output();
        let (outputs, output_call) = self.read_outputs(&mut env, result)?;
        let map = OutputMap::build(&env, program, &outputs);
        Ok(DocumentRun {
            env,
            program,
            outputs,
            output_call,
            derived: self.derived.clone(),
            log,
            map,
            source: source.to_string(),
        })
    }

    /// Run `source` in a fresh env without tracing, and read its output as
    /// plain JSON (the shape `petal::value::value_to_json` gives): what a
    /// host that only reads a document (an export, a command line) needs,
    /// without the cost of mapping it back to the source. Errors are those
    /// of [`run`](Self::run), and an output value still loading (pending).
    pub fn evaluate(&self, source: &str) -> Result<Evaluated, String> {
        let (mut env, _, stack) = self.prepare_traced(source, false)?;
        let result = env.run(stack)?;
        let log = env.take_output();
        let (outputs, output_call) = self.read_outputs(&mut env, result)?;
        let outputs = plain_json(&outputs, &env)?;
        Ok(Evaluated { outputs, output_call, log })
    }

    /// The run's output values, as [`OutputSource`] says to read them, and
    /// the output call that made them.
    fn read_outputs(&self, env: &mut Env, result: Value) -> Result<(Vec<Value>, Option<OutputCall>), String> {
        let last = |env: &mut Env, symbol: &str, n: usize| {
            let sym = env.intern_symbol(symbol);
            let mut out = env.take_output_buffer(sym);
            if out.len() < n {
                return None;
            }
            out.drain(..out.len() - n);
            Some(out)
        };
        match &self.output {
            OutputSource::ReturnValue => Ok((vec![result], None)),
            OutputSource::Call(_) | OutputSource::AnyCall(_) => {
                let calls = self.output.calls();
                for c in calls {
                    if let Some(out) = last(env, c.native, c.arity) {
                        return Ok((out, Some(*c)));
                    }
                }
                let names: Vec<&str> = calls.iter().map(|c| c.call).collect();
                Err(format!("the script never called {}", names.join(" or ")))
            }
            OutputSource::Buffer { symbol, last: None } => {
                let sym = env.intern_symbol(symbol);
                Ok((env.take_output_buffer(sym), None))
            }
            OutputSource::Buffer { symbol, last: Some(n) } => match last(env, symbol, *n) {
                Some(out) => Ok((out, None)),
                None => Err(format!("the script never handed its document to `{symbol}`")),
            },
        }
    }
}

/// `values` as plain JSON, refusing one still loading.
fn plain_json(values: &[Value], env: &Env) -> Result<Vec<serde_json::Value>, String> {
    values.iter().map(|v| petal::value::value_to_json(v, env.heap()).map_err(|e| e.to_string())).collect()
}

/// A document's output, read without tracing ([`Runner::evaluate`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluated {
    /// The output values as plain JSON: records are objects, elements
    /// `{"type": "element", "tag", "props", "children"}` (read them with
    /// [`crate::props`]).
    pub outputs: Vec<serde_json::Value>,
    /// The output call the script made, when the runner reads one.
    pub output_call: Option<OutputCall>,
    /// What the script printed.
    pub log: Vec<String>,
}

/// The native a probe source calls around an expression, to read the value
/// the expression has where it is written (see [`Runner::probe`]).
pub const PROBE: &str = "__programs_as_documents_probe";

impl Runner {
    /// Run `source`, which calls [`PROBE`]`(expr)` around one or more
    /// expressions, and return each value the probe was called with, in
    /// call order. The probe returns its argument, so the run is the
    /// document's own; its declaration is appended to the source. Errors
    /// are those of [`run`](Self::run).
    pub fn probe(&self, source: &str) -> Result<Vec<StaticValue>, String> {
        let mut env = Env::new();
        (self.setup)(&mut env)?;
        env.register_native_boxed(
            PROBE,
            |cx| {
                let v = cx.get_value(1)?;
                let sym = cx.intern_symbol(PROBE);
                cx.push_output(sym, v);
                cx.push_value(v);
                Ok(1)
            },
            petal::native_fn::NativeEffects::EMITS,
        );
        let source = format!("{source}\nnative fn {PROBE}(v)\n");
        let program = env.load_program_at(&source, &self.path)?;
        let stack = env.create_stack(program)?;
        env.run(stack)?;
        let sym = env.intern_symbol(PROBE);
        let values = env.take_output_buffer(sym);
        Ok(values.iter().map(|v| observe(v, env.heap())).collect())
    }
}

/// One run of a document: its env (which owns the heap the result values
/// live in), the result values, what it printed, and its output map.
pub struct DocumentRun {
    pub env: Env,
    pub program: ProgramId,
    /// The output values, as [`OutputSource`] says to read them.
    pub outputs: Vec<Value>,
    /// The output call the script made, when the runner reads one
    /// ([`OutputSource::Call`], [`OutputSource::AnyCall`]).
    pub output_call: Option<OutputCall>,
    /// Which of them are derived from the others ([`Runner::with_derived`]).
    pub derived: Vec<usize>,
    /// What the script printed.
    pub log: Vec<String>,
    pub map: OutputMap,
    /// The source text that was run.
    pub source: String,
}

impl DocumentRun {
    /// The compiled program.
    pub fn program(&self) -> &Program {
        self.env.get_program(self.program).expect("a run's program stays loaded")
    }

    /// The output values as plain JSON, as [`Evaluated::outputs`] has them.
    /// Errors when one is still loading (pending).
    pub fn plain_outputs(&self) -> Result<Vec<serde_json::Value>, String> {
        plain_json(&self.outputs, &self.env)
    }

    /// Each output value as a literal, or `None` for one with no literal
    /// form (a function).
    pub fn static_outputs(&self) -> Vec<Option<StaticValue>> {
        self.outputs.iter().map(|v| StaticValue::from_value(v, self.env.heap())).collect()
    }

    /// Each result value as it is observed when two runs are compared: like
    /// [`static_outputs`](Self::static_outputs), but a value with no literal
    /// form (a function, a handle) inside it becomes [`opaque`] instead of
    /// making the whole output `None`, so the rest of it is still compared.
    pub fn observed_outputs(&self) -> Vec<StaticValue> {
        self.outputs.iter().map(|v| observe(v, self.env.heap())).collect()
    }

    /// The result values as two runs are compared to verify an edit:
    /// [`observed_outputs`](Self::observed_outputs) with each derived value
    /// ([`Runner::with_derived`]) left out (as `nil`).
    pub fn compared_outputs(&self) -> Vec<StaticValue> {
        let mut out = self.observed_outputs();
        for &i in &self.derived {
            if let Some(v) = out.get_mut(i) {
                *v = StaticValue::Nil;
            }
        }
        out
    }

    /// Whether the output at index path `path` is inside a derived result
    /// value.
    pub fn is_derived(&self, path: &[crate::output_map::PathSeg]) -> bool {
        matches!(path.first(), Some(crate::output_map::PathSeg::Index(i)) if self.derived.contains(i))
    }

    /// The element node `node` is, as [`observed_outputs`](Self::observed_outputs)
    /// sees it.
    pub fn observed_node(&self, node: &OutputNode) -> StaticValue {
        observe(&Value::Element(node.element), self.env.heap())
    }

    /// The output node `key` names (see [`OutputMap::find`]).
    pub fn node(&self, key: &OutputKey) -> Option<&OutputNode> {
        self.map.find(key)
    }

    /// The element node `node` is, as a literal.
    pub fn node_value(&self, node: &OutputNode) -> Option<StaticValue> {
        StaticValue::from_value(&Value::Element(node.element), self.env.heap())
    }
}

/// What [`DocumentRun::observed_outputs`] puts in place of a value with no
/// literal form. No literal reads back as it ([`StaticValue::from_value`]
/// never makes a call), so it never equals a real value; all of them equal
/// each other, so a function prop compares as unchanged.
pub fn opaque() -> StaticValue {
    StaticValue::Call { function: "<opaque>".into(), args: Vec::new() }
}

/// `v` as a literal, with [`opaque`] for each part that has none.
fn observe(v: &Value, heap: &petal::heap::Heap) -> StaticValue {
    if let Some(s) = StaticValue::from_value(v, heap) {
        return s;
    }
    match v {
        Value::List(id) => StaticValue::List(heap.get_list(*id).iter().map(|x| observe(x, heap)).collect()),
        Value::Map(id) => StaticValue::Record(heap.get_map(*id).iter().map(|(k, x)| (k.to_string(), observe(x, heap))).collect()),
        Value::Element(id) => StaticValue::Element {
            tag: heap.get_string(heap.get_element_tag(*id)).to_string(),
            props: heap
                .get_map(heap.get_element_props(*id))
                .iter()
                .map(|(k, x)| (k.to_string(), observe(x, heap)))
                .collect(),
            children: heap.get_list(heap.get_element_children(*id)).iter().map(|x| observe(x, heap)).collect(),
        },
        Value::Cell(id) => observe(&heap.cell_read(*id), heap),
        _ => opaque(),
    }
}
