// hello.cpp — the smallest useful petal-bridge host.
//
// Registers one host callback and one emitter, runs a script for a few
// frames, and prints what the script emitted and drew.
#include <petal.hpp>

#include <cstdio>

static const char* kScript = R"petal(
state frames = 0
frames += 1

// A host callback: synchronous, returns a value.
let g = gravity_at(frames)

// An emitter: pushes spawn("ball", {...}) into the host's "scene" buffer.
spawn("ball", {x: frames * 1.5, y: g})

// petal-ui drawing.
draw_rect(10 * frames, 20, 30, 40, 255, 128, 0)
print("frame {frames}: gravity {g}")
)petal";

int main() {
    try {
        petal::Vm vm;
        vm.native("gravity_at", [](petal::Call& call) {
            call.result().value(-9.81 * call[0].number());
        });
        vm.emitter("spawn", "scene");
        vm.load_source(kScript, "hello.ptl");
        vm.set_dimensions(640, 480);

        for (int frame = 0; frame < 3; ++frame) {
            vm.begin_frame(1.0 / 60.0, frame, frame / 60.0);
            vm.run();
            for (const std::string& line : vm.take_output()) std::printf("script: %s\n", line.c_str());
            for (petal::Value cmd : vm.drain("scene")) {
                std::printf("  %.*s(%.*s) at x=%.2f y=%.2f\n", int(cmd.tag().size()), cmd.tag().data(),
                            int(cmd[0].str().size()), cmd[0].str().data(), cmd[1].num("x"), cmd[1].num("y"));
            }
            for (const pb_draw_cmd& d : vm.drain_draw()) {
                if (d.kind == PB_DRAW_RECT) std::printf("  rect %d,%d %dx%d\n", d.x, d.y, d.w, d.h);
            }
        }
        std::printf("state: %s\n", vm.state_json().c_str());
    } catch (const petal::Error& e) {
        std::fprintf(stderr, "petal error (%s) at %s:%u:%u: %s\n", pb_status_name(e.code()), e.file().c_str(),
                     e.line(), e.column(), e.what());
        return 1;
    }
    return 0;
}
