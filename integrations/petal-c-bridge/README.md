# petal-c-bridge

Embed Petal in a C or C++ program. A Rust static library owns the Petal VM
(with petal-ui input and draw commands); `include/petal_bridge.h` is its C ABI
and `include/petal.hpp` a header-only C++20 wrapper over it.

```cpp
#include <petal.hpp>

petal::Vm vm;
vm.native("gravity_at", [](petal::Call& c) { c.result().value(-9.81 * c[0].number()); });
vm.emitter("spawn", "scene");          // spawn(...) pushes into the "scene" buffer
vm.register_package("game", {{"game", R"(   // the interface package: declares the natives
pub native fn gravity_at(frame: num) -> float
pub native fn spawn(kind: string, props: record)
)"}});
vm.load_file("game.ptl");              // the script says `import game` (and `import ui`)
for (int frame = 0;; ++frame) {
    vm.begin_frame(dt, frame, t);
    vm.run();
    for (petal::Value cmd : vm.drain("scene")) { /* cmd.tag(), cmd[0], ... */ }
    for (const pb_draw_cmd& d : vm.drain_draw()) { /* rasterize */ }
    if (vm.sources_changed()) vm.reload();   // hot reload, keeping `state`
}
```

Link it from CMake with `add_subdirectory(...)` and the target
`petal::bridge`. Build and test it on its own from the repository root:

```sh
make test-c-bridge
```

That needs CMake, Ninja and a C++20 compiler. The guide is
[docs/embedding-c.md](../../docs/embedding-c.md). It covers the frame
contract, values and builders, host natives and emitters, petal-ui input and
draw commands, modules and packages (interface packages, `[requires]` and
`provides`, bundles for hosts with no filesystem), input-scenario replay, hot reload (a value or layout edit is
taken up without recompiling, and `vm.set_config(path, json)` sets a config
value with no file write) and errors.
