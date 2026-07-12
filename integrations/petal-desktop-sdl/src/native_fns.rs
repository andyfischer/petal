//! petal-sdl's host-specific natives and bindings. The standard input/draw
//! contract (mouse/key natives, draw commands, offscreen canvases, the `ui`
//! prelude module) comes from `petal-ui`; this module adds only what is
//! particular to this app: the example browser and sandboxed file I/O.

use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};
use petal::value::Value;

/// Host→script browser state: the example list, as `[name, path]` pairs.
const SYM_EXAMPLES: &str = "examples";
/// Output channel: paths the sketch asked the host to launch (`launch_script`).
const LAUNCH_SCRIPT_SIGNAL: &str = "launch_script";

pub struct ExampleEntry {
    pub name: String,
    pub path: String,
}

/// Register everything a petal-sdl script can call: the petal-ui standard
/// set (input, draw, offscreen canvases, the `ui` prelude as an implicit
/// import) plus the browser/file natives.
pub fn register_all(env: &mut Env) {
    petal_ui::input::register_input(env);
    petal_ui::draw::register_draw(env);
    petal_ui::draw::register_canvas(env);
    petal_ui::register_prelude(env);
    env.register_native("example_count", native_example_count);
    env.register_native("example_name", native_example_name);
    env.register_native("example_path", native_example_path);
    env.register_native("launch_script", native_launch_script);
    env.register_native("load_text_file", native_load_text_file);
    env.register_native("save_text_file", native_save_text_file);
    env.register_native("file_exists", native_file_exists);
}

// ── Host-side helpers ─────────────────────────────────────────────────────

/// Bind the browser example list as a list of `[name, path]` pairs.
pub fn bind_examples(env: &mut Env, examples: &[ExampleEntry]) {
    let mut pairs = Vec::with_capacity(examples.len());
    for e in examples {
        let name = Value::String(env.heap_mut().alloc_string(e.name.clone()));
        let path = Value::String(env.heap_mut().alloc_string(e.path.clone()));
        pairs.push(Value::List(env.heap_mut().alloc_list(vec![name, path])));
    }
    let list = Value::List(env.heap_mut().alloc_list(pairs));
    let s = env.intern_symbol(SYM_EXAMPLES);
    env.set_binding(s, list);
}

/// Drain the `launch_script` output buffer, returning the last requested path.
pub fn take_pending_launch(env: &mut Env) -> Option<String> {
    let s = env.intern_symbol(LAUNCH_SCRIPT_SIGNAL);
    let vals = env.take_output_buffer(s);
    vals.into_iter().rev().find_map(|v| match v {
        Value::String(id) => Some(env.heap().get_string(id).to_string()),
        _ => None,
    })
}

// ── Browser natives ───────────────────────────────────────────────────────

/// Read field `field` (0 = name, 1 = path) of example `i` from the bound list.
fn example_field(state: &mut PetalCxt, i: usize, field: usize) -> String {
    let list_id = match state.binding_named(SYM_EXAMPLES) {
        Value::List(id) => id,
        _ => return String::new(),
    };
    let pair = match state.heap().get_list(list_id).get(i).copied() {
        Some(Value::List(pid)) => pid,
        _ => return String::new(),
    };
    match state.heap().get_list(pair).get(field).copied() {
        Some(Value::String(sid)) => state.heap().get_string(sid).to_string(),
        _ => String::new(),
    }
}

fn native_example_count(state: &mut PetalCxt) -> NativeResult {
    let count = match state.binding_named(SYM_EXAMPLES) {
        Value::List(id) => state.heap().get_list(id).len() as i64,
        _ => 0,
    };
    state.push_int(count);
    Ok(1)
}

fn native_example_name(state: &mut PetalCxt) -> NativeResult {
    let i = state.get_int(1)? as usize;
    let name = example_field(state, i, 0);
    state.push_string(name);
    Ok(1)
}

fn native_example_path(state: &mut PetalCxt) -> NativeResult {
    let i = state.get_int(1)? as usize;
    let path = example_field(state, i, 1);
    state.push_string(path);
    Ok(1)
}

fn native_launch_script(state: &mut PetalCxt) -> NativeResult {
    let path = state.get_string(1)?;
    let pathv = Value::String(state.heap_mut().alloc_string(path));
    let sym = state.intern_symbol(LAUNCH_SCRIPT_SIGNAL);
    state.push_output(sym, pathv);
    state.push_nil();
    Ok(1)
}

// ── File I/O ──────────────────────────────────────────────────────────────
//
// Reads/writes are restricted to files under the working directory so Petal
// scripts can't escape out to arbitrary paths. Returns empty string on miss.

fn safe_path(path: &str) -> Option<std::path::PathBuf> {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        return None;
    }
    // Reject traversal. A single `..` component is enough to escape.
    for comp in p.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            return None;
        }
    }
    Some(std::env::current_dir().ok()?.join(p))
}

fn native_load_text_file(state: &mut PetalCxt) -> NativeResult {
    let path = state.get_string(1)?;
    let text = match safe_path(&path) {
        Some(p) => std::fs::read_to_string(&p).unwrap_or_default(),
        None => String::new(),
    };
    state.push_string(text);
    Ok(1)
}

fn native_save_text_file(state: &mut PetalCxt) -> NativeResult {
    let path = state.get_string(1)?;
    let content = state.get_string(2)?;
    let ok = match safe_path(&path) {
        Some(p) => {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&p, content).is_ok()
        }
        None => false,
    };
    state.push_bool(ok);
    Ok(1)
}

fn native_file_exists(state: &mut PetalCxt) -> NativeResult {
    let path = state.get_string(1)?;
    let exists = match safe_path(&path) {
        Some(p) => p.exists(),
        None => false,
    };
    state.push_bool(exists);
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{DrawCommand, take_draw_commands};
    use petal_ui::draw::reset_canvas_ids;

    #[test]
    fn offscreen_canvas_emits_stream_commands() {
        let mut env = Env::new();
        register_all(&mut env);
        reset_canvas_ids(&mut env);
        // create_canvas returns id 1; redirect to it, draw, then blit onto main.
        env.run_source(
            "let c = create_canvas(32, 32)\n\
             draw_to(c)\n\
             draw_rect(0, 0, 4, 4, 255, 255, 255)\n\
             draw_to_screen()\n\
             draw_canvas(c, 10, 10)",
        )
        .expect("run_source should succeed");

        let cmds = take_draw_commands(&mut env);
        assert_eq!(
            cmds,
            vec![
                DrawCommand::CreateCanvas {
                    id: 1,
                    w: 32,
                    h: 32
                },
                DrawCommand::SetTarget { id: 1 },
                DrawCommand::Rect {
                    x: 0,
                    y: 0,
                    w: 4,
                    h: 4,
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                    radius: 0
                },
                DrawCommand::SetTarget { id: 0 },
                DrawCommand::DrawCanvas {
                    id: 1,
                    x: 10,
                    y: 10
                },
            ]
        );
    }

    #[test]
    fn canvas_ids_are_stable_after_reset() {
        let mut env = Env::new();
        register_all(&mut env);

        reset_canvas_ids(&mut env);
        let a = env.run_source("create_canvas(8, 8)").expect("run ok");
        let _ = take_draw_commands(&mut env);
        reset_canvas_ids(&mut env);
        let b = env.run_source("create_canvas(8, 8)").expect("run ok");
        let _ = take_draw_commands(&mut env);

        // After a per-frame reset, the same call site yields the same id.
        assert_eq!(a, Value::Int(1));
        assert_eq!(b, Value::Int(1));
    }

    #[test]
    fn prelude_widgets_are_available_to_sketches() {
        let mut env = Env::new();
        register_all(&mut env);
        let v = env
            .run_source("point_in(5, 5, rect(0, 0, 10, 10))")
            .expect("prelude fns resolve via implicit import");
        assert_eq!(v, Value::Bool(true));
    }
}
