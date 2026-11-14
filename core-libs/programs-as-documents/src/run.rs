//! Running a document with element tracing on.
//!
//! A host describes how to run its documents once, as a [`Runner`]: the path
//! the document is loaded as (relative imports resolve from it), a setup
//! closure that registers the host's vocabulary (packages, natives), and
//! where the document's result is read from. [`Runner::run`] then runs a
//! source text in a fresh [`Env`] with element-origin tracing on and returns
//! a [`DocumentRun`]: the result values and the [`OutputMap`] that ties each
//! element in them back to the source.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use petal::env::Env;
use petal::program::{Program, ProgramId};
use petal::stack::StackKey;
use petal::static_value::StaticValue;
use petal::value::Value;

use crate::output_map::{OutputKey, OutputMap, OutputNode};

/// Where a run's result is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputSource {
    /// The value the script evaluates to (its last expression).
    ReturnValue,
    /// The values pushed to the output buffer of symbol `symbol`, the way a
    /// vocabulary package's emit native hands its document to the host
    /// (`canvas(doc, layers)` pushing both arguments). With `last: Some(n)`,
    /// only the last `n` values: the arguments of the last emit call. A run
    /// that pushed fewer than `n` is an error (the script never made the
    /// call).
    Buffer { symbol: String, last: Option<usize> },
}

/// A host's setup for an [`Env`] before a document is loaded into it.
pub type Setup = dyn Fn(&mut Env) -> Result<(), String>;

/// How a host runs its documents. Cheap to clone.
#[derive(Clone)]
pub struct Runner {
    path: PathBuf,
    setup: Rc<Setup>,
    output: OutputSource,
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner").field("path", &self.path).field("output", &self.output).finish()
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
        }
    }

    /// Read the result from `output` instead.
    pub fn with_output(mut self, output: OutputSource) -> Runner {
        self.output = output;
        self
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
        let mut env = Env::new();
        (self.setup)(&mut env)?;
        env.set_echo(false);
        env.enable_element_trace(true);
        let program = env.load_program_at(source, &self.path)?;
        let stack = env.create_stack(program)?;
        Ok((env, program, stack))
    }

    /// Run `source` in a fresh env with element tracing on, and map its
    /// result. Errors are a load (parse, compile) or run failure, or a
    /// missing emit, as a message for the user.
    pub fn run(&self, source: &str) -> Result<DocumentRun, String> {
        let (mut env, program, stack) = self.prepare(source)?;
        let result = env.run(stack)?;
        let log = env.take_output();
        let outputs = match &self.output {
            OutputSource::ReturnValue => vec![result],
            OutputSource::Buffer { symbol, last } => {
                let sym = env.intern_symbol(symbol);
                let mut out = env.take_output_buffer(sym);
                if let Some(n) = *last {
                    if out.len() < n {
                        return Err(format!("the script never handed its document to `{symbol}`"));
                    }
                    out.drain(..out.len() - n);
                }
                out
            }
        };
        let map = OutputMap::build(&env, program, &outputs);
        Ok(DocumentRun {
            env,
            program,
            outputs,
            log,
            map,
            source: source.to_string(),
        })
    }
}

/// One run of a document: its env (which owns the heap the result values
/// live in), the result values, what it printed, and its output map.
pub struct DocumentRun {
    pub env: Env,
    pub program: ProgramId,
    /// The result values, as [`OutputSource`] says to read them.
    pub outputs: Vec<Value>,
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

    /// Each result value as a literal, or `None` for one with no literal
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
