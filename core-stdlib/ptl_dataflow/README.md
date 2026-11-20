# ptl_dataflow

A Petal program drawn as its dataflow diagram, written entirely in Petal.

A Petal program has no hidden mutation: a value is computed from the values it
names, and the only things that outlive a frame are the `state` slots it
declares. So the compiler's IR already *is* a dataflow graph. This package
folds that IR into something a reader recognizes (one box per binding), lays
it out, and draws it beside the source, with the two linked.

| Module | What it does |
|---|---|
| `ptl_dataflow/extract` | `dataflow_graph(ir, natives, source)`: the user-only IR → `{nodes, clusters, edges}`, one node per `let`, `state`, `var`, parameter, host input, effect and return; one cluster per function and loop body |
| `ptl_dataflow/layout` | `layout_graph(g)`: a layered left-to-right layout with nested boxes (cycle breaking, longest-path layers, barycenter ordering, isotonic placement, orthogonal routing through per-edge channels and box ports); `collapse(g, ids)` folds boxes into one node |
| `ptl_dataflow/view` | `dataflow_viewer(r, source, compiled, opts)`: the interactive viewer, an editable highlighted code pane and the diagram, linked by hover |

```petal ignore
import ptl_dataflow

state source = START
state compiled = nil     # the host writes {source, result}
let v = dataflow_viewer(rect(0, 0, screen_width(), screen_height()), source, compiled)
source = v.source
```

## Where the IR comes from

Petal code cannot compile Petal, so the host does: petal-web-canvas's
`dataflow_ir(source)` returns `{ok, ir, natives, diagnostics}`, the same view
as `petal show-ir --json --user-only`. The viewer takes the latest result as
`compiled = {source, result}` and returns the source after the user's edits;
when the two differ, the host compiles again. petal-lang.org's
`/home-page-proposals/#dataflow` is the reference host.

What the fold still has to guess, because the IR does not say, is listed in
[docs/dev/dataflow-visualizer-gaps.md](../../docs/dev/dataflow-visualizer-gaps.md).

## Status

Experimental, and not bundled: a host registers it
(`register_package("ptl_dataflow", modules)`), or a project names it as a
path dependency. It needs the `ui` interface.
