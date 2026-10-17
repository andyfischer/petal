// bench_frame.cpp — the figures in docs/usage/embedding-c.md, "Performance notes".
//
// One frame makes 200 host-callback calls, 200 emitter calls and 2 draw calls,
// then the host drains and decodes everything. Prints the minimum and median
// frame time, the time to load the program (which imports `ui`) and the time
// of a reload that recompiles it. Build the bridge in release (the default)
// and run `build/petal_bridge_bench_frame`; it is not a test, since the
// numbers depend on the machine.
#include <petal.hpp>
#include <chrono>
#include <cstdio>
#include <algorithm>
#include <vector>

static const char* kInterface = R"petal(
pub native fn probe(i: num) -> float
pub native fn spawn(kind: string, props: record)
)petal";

static const char* kScript = R"petal(
import ui
import bench
state frames = 0
frames += 1
var acc = 0.0
for i in range(0, 200) do
  set acc = acc + probe(i + frames)
  spawn("ball", {x: i * 1.5, y: frames})
end
draw_rect(10, 20, 30, 40, 255, 128, 0)
draw_text("acc " ++ str(acc), 5, 5, 14, 255, 255, 255)
)petal";

using clk = std::chrono::steady_clock;
static double ms(clk::duration d) { return std::chrono::duration<double, std::milli>(d).count(); }

int main() {
    long calls = 0;
    std::vector<double> loads, reloads, frames;
    for (int round = 0; round < 7; ++round) {
        petal::Vm vm;
        vm.native("probe", [&](petal::Call& call) { ++calls; call.result().value(call[0].number() * 0.5); });
        vm.emitter("spawn", "scene");
        vm.register_package("bench", {{"bench", kInterface}});
        auto t0 = clk::now();
        vm.load_source(kScript, "bench.ptl");
        loads.push_back(ms(clk::now() - t0));
        vm.set_dimensions(640, 480);
        for (int f = 0; f < 300; ++f) {
            auto a = clk::now();
            vm.begin_frame(1.0 / 60.0, f, f / 60.0);
            vm.run();
            double sum = 0; size_t n = 0;
            for (petal::Value cmd : vm.drain("scene")) { sum += cmd[1].num("x"); ++n; }
            auto draws = vm.drain_draw();
            frames.push_back(ms(clk::now() - a));
            if (n != 200 || draws.size() != 2) { std::fprintf(stderr, "bad frame %zu %zu\n", n, draws.size()); return 1; }
        }
        std::string edited = std::string(kScript) + "\nlet extra = 1\n";
        auto r0 = clk::now();
        vm.reload_source(edited);
        reloads.push_back(ms(clk::now() - r0));
    }
    auto stat = [](std::vector<double> v, const char* name) {
        std::sort(v.begin(), v.end());
        std::printf("%s: min %.3f ms, median %.3f ms (n=%zu)\n", name, v[0], v[v.size() / 2], v.size());
    };
    stat(frames, "frame (200 callbacks, 200 emits, 2 draws, drain+decode)");
    stat(loads, "load");
    stat(reloads, "reload (recompile)");
    std::printf("callbacks %ld (expect %d)\n", calls, 7 * 300 * 200);
}
