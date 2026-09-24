// petal-c-bridge tests: the C ABI and the C++ wrapper against real scripts.
//
// Uses the header-only runner in test_harness.hpp. Run all tests, or pass
// substrings to run a subset:
//   petal_bridge_tests            # everything
//   petal_bridge_tests reload     # tests whose name contains "reload"
#include <petal.hpp>

#include "test_harness.hpp"

#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>

namespace fs = std::filesystem;

// Run `stmt`, expecting a petal::Error with status `want`; yields the error.
#define CHECK_THROWS_CODE(stmt, want)                                                            \
    [&]() -> petal::Error {                                                                      \
        try {                                                                                    \
            stmt;                                                                                \
        } catch (const petal::Error& e) {                                                        \
            if (e.code() != (want))                                                              \
                petal_test::fail(__FILE__, __LINE__, std::string(#stmt " threw ") +     \
                                                                  pb_status_name(e.code()) + ": " + e.what()); \
            return e;                                                                            \
        }                                                                                        \
        petal_test::fail(__FILE__, __LINE__, #stmt " did not throw");                   \
        return petal::Error(PB_OK, "");                                                          \
    }()

// ─── Helpers ────────────────────────────────────────────────────────────────

static fs::path scratch_dir(const std::string& test) {
    fs::path dir = fs::path(PETAL_TEST_SCRATCH) / test;
    fs::remove_all(dir);
    fs::create_directories(dir);
    return dir;
}

// Write a file and push its mtime forward, so a same-second rewrite still
// reads as a change on filesystems with coarse timestamps.
static void write_file(const fs::path& p, const std::string& text) {
    fs::file_time_type before{};
    bool existed = fs::exists(p);
    if (existed) before = fs::last_write_time(p);
    { std::ofstream(p) << text; }
    if (existed && fs::last_write_time(p) <= before) fs::last_write_time(p, before + std::chrono::seconds(1));
}

// Run one frame and return the numbers pushed to buffer `out`.
static std::vector<double> run_out(petal::Vm& vm, const char* buffer = "out") {
    vm.run();
    std::vector<double> v;
    for (petal::Value x : vm.drain(buffer)) v.push_back(x.number());
    return v;
}

// ─── Loading and running ────────────────────────────────────────────────────

TEST(load_and_run) {
    petal::Vm vm;
    CHECK(!vm.loaded());
    vm.load_source("print(\"hello from petal\")\nprint(1 + 2)");
    CHECK(vm.loaded());
    vm.run();
    auto lines = vm.take_output();
    REQUIRE(lines.size() == 2);
    CHECK_EQ(lines[0], std::string("hello from petal"));
    CHECK_EQ(lines[1], std::string("3"));
    CHECK(vm.take_output().empty());
}

TEST(run_without_program_fails) {
    petal::Vm vm;
    CHECK_THROWS_CODE(vm.run(), PB_ERR_NOT_LOADED);
}

TEST(load_file_from_disk) {
    fs::path dir = scratch_dir("load_file");
    write_file(dir / "main.ptl", "push_output(symbol(\"out\"), 42)\n");
    petal::Vm vm;
    vm.load_file((dir / "main.ptl").string());
    CHECK_EQ(run_out(vm), std::vector<double>{42});
    CHECK_THROWS_CODE(vm.load_file((dir / "missing.ptl").string()), PB_ERR_IO);
}

TEST(state_persists_across_frames) {
    petal::Vm vm;
    vm.load_source(
        "state n = 0\n"
        "n += 1\n"
        "state total = 0.0\n"
        "total = total + 0.5\n"
        "push_output(symbol(\"out\"), n)\n");
    CHECK_EQ(run_out(vm), std::vector<double>{1});
    CHECK_EQ(run_out(vm), std::vector<double>{2});
    CHECK_EQ(run_out(vm), std::vector<double>{3});
    CHECK_EQ(vm.state("n").integer(), 3);
    CHECK_NEAR(vm.state("total").number(), 1.5, 1e-3);
    CHECK(vm.state_json().find("\"n\":3") != std::string::npos);
    CHECK_THROWS_CODE(vm.state("nope"), PB_ERR_NOT_FOUND);

    // Loading again starts over with a fresh stack.
    vm.load_source("state n = 100\nn += 1\npush_output(symbol(\"out\"), n)\n");
    CHECK_EQ(run_out(vm), std::vector<double>{101});
}

TEST(memo_switch_keeps_output_and_state) {
    petal::Vm vm;
    vm.load_source(
        "fn counter(step)\n"
        "  state n = 0\n"
        "  n += step\n"
        "  n\n"
        "end\n"
        "for i in range(0, 2) do push_output(symbol(\"out\"), counter(i + 1)) end\n"
        "push_output(symbol(\"out\"), counter(10))\n");
    vm.set_memo(true);
    CHECK(vm.memo());
    CHECK_EQ(run_out(vm), (std::vector<double>{1, 2, 10}));
    // Switching memo off (and back on) between runs changes nothing a
    // program can see: every call site keeps its own state slot.
    vm.set_memo(false);
    CHECK(!vm.memo());
    CHECK_EQ(run_out(vm), (std::vector<double>{2, 4, 20}));
    CHECK_EQ(run_out(vm), (std::vector<double>{3, 6, 30}));
    vm.set_memo(true);
    CHECK_EQ(run_out(vm), (std::vector<double>{4, 8, 40}));
}

TEST(profiler_reports_functions_and_natives) {
    petal::Vm vm;
    vm.load_source(
        "fn spin(n)\n"
        "  let acc = 0.0\n"
        "  for i in range(0, n) do acc = acc + sqrt(float(i)) end\n"
        "  acc\n"
        "end\n"
        "push_output(symbol(\"out\"), spin(50))\n");
    vm.set_profiling(true);
    vm.run();
    std::string report = vm.profile_report(10);
    CHECK(report.find("top functions") != std::string::npos);
    CHECK(report.find("spin") != std::string::npos);
    CHECK(report.find("natives by time") != std::string::npos);
    CHECK(report.find("sqrt") != std::string::npos);
    // Turning it back on starts a fresh measurement.
    vm.set_profiling(true);
    CHECK(vm.profile_report(10).find("spin") == std::string::npos);
}

TEST(restart_starts_state_over_without_recompiling) {
    petal::Vm vm;
    CHECK_THROWS_CODE(vm.restart(), PB_ERR_NOT_LOADED);
    vm.load_source("state n = 0\nn += 1\npush_output(symbol(\"out\"), n)\n");
    CHECK_EQ(run_out(vm), std::vector<double>{1});
    CHECK_EQ(run_out(vm), std::vector<double>{2});
    vm.restart();
    CHECK_EQ(run_out(vm), std::vector<double>{1});
}

TEST(buffers_are_cleared_each_run) {
    petal::Vm vm;
    vm.load_source("push_output(symbol(\"out\"), 1)\n");
    vm.run();
    vm.run();  // the first run's value was never drained
    CHECK_EQ(vm.drain("out").size(), size_t(1));
    CHECK_EQ(vm.drain("out").size(), size_t(0));  // drained
}

// ─── Bindings ───────────────────────────────────────────────────────────────

TEST(bindings_reach_the_script) {
    petal::Vm vm;
    vm.load_source(
        "fn b(name) binding(symbol(name)) end\n"
        "push_output(symbol(\"out\"), b(\"f\"))\n"
        "push_output(symbol(\"out\"), b(\"i\"))\n"
        "push_output(symbol(\"out\"), b(\"flag\"))\n"
        "push_output(symbol(\"out\"), b(\"name\"))\n"
        "push_output(symbol(\"out\"), b(\"v2\"))\n"
        "push_output(symbol(\"out\"), b(\"v3\").z)\n"
        "push_output(symbol(\"out\"), b(\"fs\")[2])\n"
        "push_output(symbol(\"out\"), b(\"snap\").bodies[1].pos.y)\n"
        "push_output(symbol(\"out\"), b(\"missing\"))\n");
    vm.set_float("f", 2.5);
    vm.set_int("i", -7);
    vm.set_bool("flag", true);
    vm.set_string("name", "marble");
    vm.set_vec2("v2", 3, 4);
    vm.set_vec3("v3", 1, 2, 3);
    std::vector<double> fs = {0.5, 1.5, 2.5};
    vm.set_floats("fs", fs);
    vm.set_value("snap", [](petal::BuilderRef& b) {
        b.map([](petal::BuilderRef& m) {
            m.key("bodies").list([](petal::BuilderRef& l) {
                l.map([](petal::BuilderRef& body) { body.field("id", "a").key("pos").vec3(0, 1, 0); });
                l.map([](petal::BuilderRef& body) { body.field("id", "b").key("pos").vec3(0, 9.5, 0); });
            });
        });
    });
    vm.run();
    petal::Values out = vm.drain("out");
    REQUIRE(out.size() == 9);
    CHECK(out[0].is_float());
    CHECK_NEAR(out[0].number(), 2.5, 1e-3);
    CHECK(out[1].is_int());
    CHECK_EQ(out[1].integer(), -7);
    CHECK(out[2].is_bool() && out[2].boolean());
    CHECK_EQ(out[3].str(), std::string_view("marble"));
    CHECK(out[4].is_vec2());
    CHECK_NEAR(out[4].x(), 3.0, 1e-3);
    CHECK_NEAR(out[4].y(), 4.0, 1e-3);
    CHECK_NEAR(out[5].number(), 3.0, 1e-3);
    CHECK_NEAR(out[6].number(), 2.5, 1e-3);
    CHECK_NEAR(out[7].number(), 9.5, 1e-3);
    CHECK(out[8].is_nil());

    // Bindings persist until changed or cleared.
    vm.clear_binding("f");
    vm.run();
    CHECK(vm.drain("out")[0].is_nil());
}

TEST(vec3_is_native_both_ways) {
    petal::Vm vm;
    // A host native that takes and returns native vec3s.
    vm.native("host_up", [](petal::Call& c) {
        REQUIRE(c.size() == 1);
        petal::Value v = c[0];
        CHECK(v.is_vec3());
        c.result().vec3(v.x(), v.y() + 1, v.z());
    });
    vm.set_vec3("sun", 0.5, -1, 0.25);
    vm.load_source(
        "let out = symbol(\"out\")\n"
        "let sun = binding(symbol(\"sun\"))\n"
        "push_output(out, sun)\n"
        "push_output(out, sun + vec3(1, 1, 1))\n"               // operators work: it is a real vec3
        "push_output(out, cross(vec3(1, 0, 0), vec3(0, 1, 0)))\n"
        "push_output(out, host_up(vec3(1, 2, 3)))\n"
        "push_output(out, {pos: vec3(7, 8, 9), rec: {x: 1, y: 2, z: 3}})\n");
    vm.run();
    petal::Values out = vm.drain("out");
    REQUIRE(out.size() == 5);
    CHECK(out[0].is_vec3());
    CHECK(!out[0].is_vec2() && !out[0].is_map() && out[0].empty());
    CHECK_NEAR(out[0].x(), 0.5, 1e-9);
    CHECK_NEAR(out[0].y(), -1.0, 1e-9);
    CHECK_NEAR(out[0].z(), 0.25, 1e-9);
    CHECK_NEAR(out[1].z(), 1.25, 1e-9);
    CHECK(out[2].is_vec3());
    CHECK_NEAR(out[2].z(), 1.0, 1e-9);
    CHECK(out[3].is_vec3());
    CHECK_NEAR(out[3].y(), 3.0, 1e-9);
    CHECK_NEAR(out[3].z(), 3.0, 1e-9);
    CHECK(out[4]["pos"].is_vec3());
    CHECK_NEAR(out[4]["pos"].z(), 9.0, 1e-9);
    // An {x, y, z} record is still a record.
    CHECK(out[4]["rec"].is_map());
    CHECK_NEAR(out[4]["rec"].num("z"), 3.0, 1e-9);
    // num("x"/"y"/"z") reads a vector's components the way a record's fields
    // read, so host code written against {x, y, z} records accepts vectors.
    CHECK_NEAR(out[4]["pos"].num("z"), 9.0, 1e-9);
    CHECK_NEAR(out[0].num("x"), 0.5, 1e-9);
    CHECK_NEAR(out[0].num("w", -7.0), -7.0, 1e-9);
    CHECK(!out[4]["pos"].has("z"));  // get()/has() stay map-only
    // A missing value reads as zero components.
    CHECK_NEAR(petal::Value().z(), 0.0, 1e-9);
}

TEST(builder_misuse_is_reported) {
    petal::Vm vm;
    petal::Builder b;
    b.key("oops");  // key outside a map
    b.value(1.0);
    CHECK_THROWS_CODE(vm.set_value("x", b), PB_ERR_INVALID_ARG);

    petal::Builder unbalanced;
    unbalanced.begin_list().value(1.0);
    CHECK_THROWS_CODE(vm.set_value("x", unbalanced), PB_ERR_INVALID_ARG);

    petal::Builder two;
    two.value(1.0).value(2.0);
    CHECK_THROWS_CODE(vm.set_value("x", two), PB_ERR_INVALID_ARG);
}

// ─── Output buffers decode ──────────────────────────────────────────────────

TEST(buffers_decode_nested_values) {
    petal::Vm vm;
    vm.load_source(
        "enum Shape\n"
        "  Circle(radius),\n"
        "  Box(w, h),\n"
        "  Empty,\n"
        "end\n"
        "let out = symbol(\"out\")\n"
        "push_output(out, {\n"
        "  name: \"crate\",\n"
        "  pos: {x: 1.5, y: 2, z: -3},\n"
        "  tags: [\"wood\", \"heavy\"],\n"
        "  shape: Box(2, 3),\n"
        "  inner: [[1, 2], [], [Circle(0.5)]],\n"
        "  vel: vec2(1, -1),\n"
        "  nothing: nil,\n"
        "  on: false,\n"
        "})\n"
        "push_output(out, Circle(4))\n"
        "push_output(out, Empty)\n"
        "push_output(out, [1, 2.5, \"three\"])\n");
    vm.run();
    petal::Values out = vm.drain("out");
    REQUIRE(out.size() == 4);

    petal::Value rec = out[0];
    REQUIRE(rec.is_map());
    CHECK_EQ(rec.size(), size_t(8));
    CHECK_EQ(rec["name"].str(), std::string_view("crate"));
    CHECK_NEAR(rec["pos"]["x"].number(), 1.5, 1e-3);
    CHECK_EQ(rec["pos"]["y"].integer(), 2);
    CHECK_NEAR(rec["pos"].num("z"), -3.0, 1e-3);
    CHECK_EQ(rec["pos"][0].key(), std::string_view("x"));  // fields keep order
    CHECK_EQ(rec["tags"].size(), size_t(2));
    CHECK_EQ(rec["tags"][1].str(), std::string_view("heavy"));
    CHECK_EQ(rec["shape"].tag(), std::string_view("Box"));
    CHECK_EQ(rec["shape"][1].integer(), 3);
    CHECK_EQ(rec["inner"][0][1].integer(), 2);
    CHECK(rec["inner"][1].is_list() && rec["inner"][1].empty());
    CHECK_EQ(rec["inner"][2][0].tag(), std::string_view("Circle"));
    CHECK_NEAR(rec["inner"][2][0][0].number(), 0.5, 1e-3);
    CHECK(rec["vel"].is_vec2());
    CHECK(rec.has("nothing") && rec["nothing"].is_nil());
    CHECK(!rec.has("absent"));
    CHECK(rec["on"].is_bool() && !rec["on"].boolean());
    CHECK(rec["absent"]["deeper"][3].is_nil());  // chained misses are safe

    CHECK_EQ(out[1].tag(), std::string_view("Circle"));
    CHECK_EQ(out[1][0].integer(), 4);
    CHECK_EQ(out[2].tag(), std::string_view("Empty"));
    CHECK_EQ(out[2].size(), size_t(0));

    // Iteration over list items.
    std::vector<std::string> kinds;
    for (petal::Value v : out[3]) kinds.push_back(v.is_int() ? "int" : v.is_float() ? "float" : "string");
    CHECK_EQ(kinds.size(), size_t(3));
    CHECK(kinds == (std::vector<std::string>{"int", "float", "string"}));
}

TEST(views_survive_multiple_drains_in_one_frame) {
    petal::Vm vm;
    vm.load_source(
        "push_output(symbol(\"a\"), {k: \"first\"})\n"
        "push_output(symbol(\"b\"), {k: \"second\"})\n");
    vm.run();
    petal::Values a = vm.drain("a");
    petal::Values b = vm.drain("b");
    CHECK_EQ(a[0]["k"].str(), std::string_view("first"));
    CHECK_EQ(b[0]["k"].str(), std::string_view("second"));
}

// ─── Host natives ───────────────────────────────────────────────────────────

TEST(host_callback_returns_values) {
    petal::Vm vm;
    int calls = 0;
    vm.native("host_add", petal::fx::Pure, [&](petal::Call& c) {
        ++calls;
        CHECK_EQ(c.name(), std::string_view("host_add"));
        REQUIRE(c.size() == 2);
        c.result().value(c[0].number() + c[1].number());
    });
    // A raycast-shaped query: record in, record out.
    vm.native("raycast", petal::fx::ReadsHostData, [&](petal::Call& c) {
        petal::Value from = c[0], dir = c[1];
        double max = c[2].number(100);
        double dist = from.num("y") / -dir.num("y");  // hit the y = 0 plane
        if (dist < 0 || dist > max) return;            // no result -> nil
        c.result().map([&](petal::BuilderRef& m) {
            m.field("dist", dist);
            m.key("point").vec3(from.num("x") + dir.num("x") * dist, 0, from.num("z") + dir.num("z") * dist);
            m.field("body", "ground");
        });
    });
    vm.native("names", [](petal::Call& c) {
        c.result().list([&](petal::BuilderRef& l) {
            for (petal::Value v : c[0]) l.value(std::string("<") + v.string() + ">");
        });
    });
    vm.native("nothing", [](petal::Call&) {});
    vm.load_source(
        "let out = symbol(\"out\")\n"
        "push_output(out, host_add(2, 3.5))\n"
        "let hit = raycast({x: 1, y: 10, z: 0}, {x: 0, y: -1, z: 0.5}, 50)\n"
        "push_output(out, hit.dist)\n"
        "push_output(out, hit.point.z)\n"
        "push_output(out, hit.body)\n"
        "push_output(out, raycast({x: 0, y: 10, z: 0}, {x: 0, y: 1, z: 0}, 50))\n"
        "push_output(out, names([\"a\", \"b\"]))\n"
        "push_output(out, nothing())\n");
    vm.run();
    petal::Values out = vm.drain("out");
    REQUIRE(out.size() == 7);
    CHECK_NEAR(out[0].number(), 5.5, 1e-3);
    CHECK_NEAR(out[1].number(), 10.0, 1e-3);
    CHECK_NEAR(out[2].number(), 5.0, 1e-3);
    CHECK_EQ(out[3].str(), std::string_view("ground"));
    CHECK(out[4].is_nil());
    CHECK_EQ(out[5][1].str(), std::string_view("<b>"));
    CHECK(out[6].is_nil());
    CHECK_EQ(calls, 1);

    // Callbacks keep working across frames and after reload.
    vm.run();
    CHECK_EQ(calls, 2);
}

TEST(host_callback_errors_fail_the_run) {
    petal::Vm vm;
    vm.native("explode", [](petal::Call&) { throw std::runtime_error("kaboom from C++"); });
    vm.load_source("state n = 0\nn += 1\nexplode()\n");
    petal::Error e = CHECK_THROWS_CODE(vm.run(), PB_ERR_RUNTIME);
    CHECK(std::string(e.what()).find("kaboom from C++") != std::string::npos);
    CHECK_EQ(e.phase(), std::string("runtime"));
}

TEST(natives_are_per_vm) {
    // Two VMs register the same name with different callbacks: dispatch goes
    // through each Env's own table, never a global.
    petal::Vm a, b;
    a.native("who", [](petal::Call& c) { c.result().value("a"); });
    b.native("filler", [](petal::Call&) {});  // b registers more natives than a
    b.native("who", [](petal::Call& c) { c.result().value("b"); });
    const char* src = "push_output(symbol(\"out\"), who())\n";
    a.load_source(src);
    b.load_source(src);
    a.run();
    b.run();
    CHECK_EQ(a.drain("out")[0].str(), std::string_view("a"));
    CHECK_EQ(b.drain("out")[0].str(), std::string_view("b"));
}

TEST(native_userdata_is_freed) {
    static int freed = 0;
    freed = 0;
    {
        pb_vm* vm = pb_vm_create();
        auto cb = [](pb_call*, void*) -> int { return 0; };
        auto fr = [](void*) { ++freed; };
        CHECK_EQ(pb_vm_register_native(vm, "x", cb, nullptr, fr, PB_FX_PURE), PB_OK);
        CHECK_EQ(pb_vm_register_native(vm, "y", cb, nullptr, fr, PB_FX_EFFECT), PB_OK);
        // A failed registration frees immediately.
        CHECK_EQ(pb_vm_register_native(vm, nullptr, cb, nullptr, fr, PB_FX_PURE), PB_ERR_INVALID_ARG);
        CHECK_EQ(freed, 1);
        pb_vm_destroy(vm);
    }
    CHECK_EQ(freed, 3);
}

TEST(reentrant_calls_are_rejected) {
    petal::Vm vm;
    pb_status inner = PB_OK;
    pb_vm* raw = vm.raw();
    vm.native("sneaky", [&](petal::Call&) { inner = pb_vm_run(raw); });
    vm.load_source("sneaky()\n");
    vm.run();
    CHECK_EQ(inner, PB_ERR_REENTRANT);
}

TEST(emitters_push_tagged_commands) {
    petal::Vm vm;
    vm.emitter("spawn", "scene");
    vm.emitter("light", "scene", "point_light");
    vm.emitter("sfx", "audio");
    vm.load_source(
        "let r = spawn(\"cube\", {x: 1, y: 2, z: 3}, 0.5)\n"
        "light({x: 0, y: 5, z: 0}, #ffcc00, 2.0)\n"
        "sfx(\"jump\")\n"
        "spawn(\"sphere\")\n"
        "push_output(symbol(\"out\"), r)\n");
    vm.run();
    petal::Values scene = vm.drain("scene");
    REQUIRE(scene.size() == 3);
    CHECK_EQ(scene[0].tag(), std::string_view("spawn"));
    CHECK_EQ(scene[0].size(), size_t(3));
    CHECK_EQ(scene[0][0].str(), std::string_view("cube"));
    CHECK_NEAR(scene[0][1].num("z"), 3.0, 1e-3);
    CHECK_NEAR(scene[0][2].number(), 0.5, 1e-3);
    CHECK_EQ(scene[1].tag(), std::string_view("point_light"));
    CHECK_EQ(scene[1][1].num("r"), 255.0);  // color literals are {r, g, b} records
    CHECK_EQ(scene[1][1].num("g"), 204.0);
    CHECK_EQ(scene[2].tag(), std::string_view("spawn"));
    CHECK_EQ(scene[2].size(), size_t(1));
    petal::Values audio = vm.drain("audio");
    REQUIRE(audio.size() == 1);
    CHECK_EQ(audio[0][0].str(), std::string_view("jump"));
    CHECK(vm.drain("out")[0].is_nil());  // emitters return nil
}

TEST(many_natives_register) {
    // Each host native is its own boxed Petal native: no fixed pool.
    petal::Vm vm;
    const int n = 3000;
    for (int i = 0; i < n; ++i) {
        if (i % 2) vm.emitter("e" + std::to_string(i), "buf");
        else vm.native("c" + std::to_string(i), [i](petal::Call& c) { c.result().value(i); });
    }
    vm.load_source(
        "e2999(1)\n"
        "push_output(symbol(\"out\"), c0() + c2998())\n");
    vm.run();
    CHECK_EQ(vm.drain("buf").size(), size_t(1));
    CHECK_EQ(vm.drain("out")[0].integer(), 2998);
}

TEST(native_userdata_outlives_reload) {
    // The callback's captures belong to the VM, not to one program: a reload
    // keeps using them, and they are freed exactly once, with the VM.
    static int freed = 0;
    freed = 0;
    {
        pb_vm* vm = pb_vm_create();
        static int calls = 0;
        calls = 0;
        auto cb = [](pb_call* call, void* ud) -> int {
            ++calls;
            pb_builder_int(pb_call_result(call), *static_cast<int*>(ud));
            return 0;
        };
        auto fr = [](void* ud) { ++freed; delete static_cast<int*>(ud); };
        CHECK_EQ(pb_vm_register_native(vm, "answer", cb, new int(42), fr, PB_FX_PURE), PB_OK);
        CHECK_EQ(pb_vm_load_source(vm, "push_output(symbol(\"out\"), answer())\n", nullptr), PB_OK);
        CHECK_EQ(pb_vm_run(vm), PB_OK);
        pb_reload_result r;
        CHECK_EQ(pb_vm_reload_source(vm, "push_output(symbol(\"out\"), answer() + 1)\n", &r), PB_OK);
        CHECK_EQ(pb_vm_run(vm), PB_OK);
        pb_values out;
        CHECK_EQ(pb_vm_drain(vm, "out", &out), PB_OK);
        REQUIRE(out.count == 1);
        CHECK_EQ(out.items[0].integer, 43);
        CHECK_EQ(calls, 2);
        CHECK_EQ(freed, 0);
        pb_vm_destroy(vm);
    }
    CHECK_EQ(freed, 1);
}

// ─── Calling functions ──────────────────────────────────────────────────────

TEST(call_function_by_name) {
    petal::Vm vm;
    vm.load_source(
        "fn add(a, b) a + b end\n"
        "fn describe(r) \"{r.name} is {r.age}\" end\n"
        "fn pair(x) [x, x * 2] end\n");
    vm.run();
    petal::Builder args;
    args.value(2).value(3);
    CHECK_EQ(vm.call("add", args).integer(), 5);

    petal::Builder rec;
    rec.map([](petal::BuilderRef& m) { m.field("name", "Ada").field("age", 36); });
    CHECK_EQ(vm.call("describe", rec).string(), std::string("Ada is 36"));

    petal::Builder one;
    one.value(1.5);
    petal::Value p = vm.call("pair", one);
    CHECK_NEAR(p[1].number(), 3.0, 1e-3);

    CHECK_THROWS_CODE(vm.call("no_such_fn"), PB_ERR_NOT_FOUND);
}

TEST(missing_hook_differs_from_failing_hook) {
    // A host calling optional hooks tells "not defined" (NOT_FOUND, or
    // has_function() == false) apart from "defined and failed" (RUNTIME).
    petal::Vm vm;
    vm.load_source(
        "fn on_key(k)\n  assert(k != \"boom\", \"hook failed\")\n  k\nend\n");
    CHECK(!vm.has_function("on_key"));  // not before the first run
    vm.run();
    CHECK(vm.has_function("on_key"));
    CHECK(!vm.has_function("on_tick"));
    petal::Builder ok;
    ok.value("a");
    CHECK_EQ(vm.call("on_key", ok).str(), std::string_view("a"));
    petal::Error missing = CHECK_THROWS_CODE(vm.call("on_tick"), PB_ERR_NOT_FOUND);
    CHECK_CONTAINS(std::string(missing.what()), "on_tick");
    petal::Builder boom;
    boom.value("boom");
    petal::Error failed = CHECK_THROWS_CODE(vm.call("on_key", boom), PB_ERR_RUNTIME);
    CHECK_CONTAINS(std::string(failed.what()), "hook failed");
    CHECK_EQ(failed.phase(), std::string("runtime"));
}

// ─── petal-ui ───────────────────────────────────────────────────────────────

TEST(ui_draw_follows_mouse) {
    petal::Vm vm;
    vm.set_dimensions(800, 600);
    vm.load_source(
        "clear(10, 20, 30)\n"
        "draw_rect(mouse_x(), mouse_y(), 20, 30, 255, 0, 0)\n"
        "draw_rect_rounded(1, 2, 3, 4, 5, 6, 7, 8, 128)\n"
        "draw_line(0, 0, screen_width(), screen_height(), 1, 2, 3)\n"
        "draw_circle(50, 60, 7, 9, 9, 9)\n"
        "draw_text(\"hi there\", 5, 6, 16, 200, 200, 200)\n"
        "fill_poly([[0, 0], [10, 0], [5, 8]], 1, 1, 1)\n"
        "fill_arc(10, 10, 2.0, 5.0, 0.0, 1.5, 4, 5, 6)\n"
        "clip_push(0, 0, 100, 100)\n"
        "clip_pop()\n");

    vm.mouse_move(120, 45);
    vm.begin_frame(1.0 / 60, 0, 0.0);
    vm.run();
    auto cmds = vm.drain_draw();
    REQUIRE(cmds.size() == 10);
    CHECK_EQ(cmds[0].kind, uint32_t(PB_DRAW_CLEAR));
    CHECK_EQ(int(cmds[0].color.b), 30);
    const pb_draw_cmd& rect = cmds[1];
    CHECK_EQ(rect.kind, uint32_t(PB_DRAW_RECT));
    CHECK_EQ(rect.x, 120);
    CHECK_EQ(rect.y, 45);
    CHECK_EQ(rect.w, 20);
    CHECK_EQ(rect.h, 30);
    CHECK_EQ(int(rect.color.r), 255);
    CHECK_EQ(int(rect.color.a), 255);
    CHECK_EQ(rect.radius, 0);
    CHECK_EQ(cmds[2].kind, uint32_t(PB_DRAW_RECT));
    CHECK_EQ(cmds[2].radius, 5);
    CHECK_EQ(int(cmds[2].color.a), 128);
    CHECK_EQ(cmds[3].kind, uint32_t(PB_DRAW_LINE));
    CHECK_EQ(cmds[3].x2, 800);
    CHECK_EQ(cmds[3].y2, 600);
    CHECK_EQ(cmds[4].kind, uint32_t(PB_DRAW_CIRCLE));
    CHECK_EQ(cmds[4].radius, 7);
    CHECK_EQ(cmds[4].cx, 50);
    CHECK_EQ(cmds[5].kind, uint32_t(PB_DRAW_TEXT));
    CHECK_EQ(std::string(cmds[5].text), std::string("hi there"));
    CHECK_EQ(cmds[5].text_len, size_t(8));
    CHECK_EQ(int(cmds[5].size), 16);
    CHECK(cmds[5].font == nullptr);
    CHECK_EQ(cmds[6].kind, uint32_t(PB_DRAW_POLY));
    REQUIRE(cmds[6].point_count == 3);
    CHECK_EQ(cmds[6].points[4], 5);
    CHECK_EQ(cmds[6].points[5], 8);
    CHECK_EQ(cmds[7].kind, uint32_t(PB_DRAW_ARC));
    CHECK_NEAR(double(cmds[7].r_out), 5.0, 1e-3);
    CHECK_NEAR(double(cmds[7].a1), 1.5, 1e-3);
    CHECK_EQ(cmds[8].kind, uint32_t(PB_DRAW_CLIP_PUSH));
    CHECK_EQ(cmds[8].w, 100);
    CHECK_EQ(cmds[9].kind, uint32_t(PB_DRAW_CLIP_POP));

    // Next frame, the rect follows the mouse.
    vm.mouse_move(300, 200);
    vm.begin_frame(1.0 / 60, 1, 1.0 / 60);
    vm.run();
    auto cmds2 = vm.drain_draw();
    REQUIRE(cmds2.size() == 10);
    CHECK_EQ(cmds2[1].x, 300);
    CHECK_EQ(cmds2[1].y, 200);
}

TEST(ui_prelude_widgets_draw) {
    // The `ui` prelude is an implicit import: button() works with no import.
    petal::Vm vm;
    vm.set_dimensions(400, 300);
    vm.load_source(
        "state clicks = 0\n"
        "if button({x: 10, y: 10, w: 100, h: 30}, \"Press\") then clicks += 1 end\n");
    vm.begin_frame(0.016, 0, 0);
    vm.run();
    bool saw_text = false;
    for (const pb_draw_cmd& d : vm.drain_draw())
        if (d.kind == PB_DRAW_TEXT && std::string(d.text) == "Press") saw_text = true;
    CHECK(saw_text);

    // Click inside the button: press and release on consecutive frames.
    vm.mouse_move(50, 20);
    vm.mouse_button(PB_MOUSE_LEFT, true);
    vm.begin_frame(0.016, 1, 0.016);
    vm.run();
    vm.mouse_button(PB_MOUSE_LEFT, false);
    vm.begin_frame(0.016, 2, 0.032);
    vm.run();
    CHECK_EQ(vm.state("clicks").integer(), 1);
}

TEST(ui_keyboard_and_relative_mouse) {
    petal::Vm vm;
    vm.load_source(
        "let out = symbol(\"out\")\n"
        "push_output(out, if key_down(\"space\") then 1 else 0 end)\n"
        "push_output(out, if key_pressed(\"w\") then 1 else 0 end)\n"
        "push_output(out, mouse_dx())\n"
        "push_output(out, mouse_dy())\n"
        "push_output(out, dt())\n"
        "push_output(out, frame_count())\n"
        "push_output(out, time())\n"
        "push_output(out, text_input())\n"
        "if key_pressed(\"g\") then grab_mouse() end\n"
        "if key_pressed(\"escape\") then release_mouse() end\n");

    vm.key("space", true);
    vm.key("w", true);
    vm.mouse_motion(5, -3);
    vm.mouse_motion(2, 1);
    vm.text("hé");
    vm.begin_frame(0.25, 7, 12.5);
    vm.run();
    petal::Values out = vm.drain("out");
    REQUIRE(out.size() == 8);
    CHECK_EQ(out[0].integer(), 1);
    CHECK_EQ(out[1].integer(), 1);
    CHECK_EQ(out[2].integer(), 7);
    CHECK_EQ(out[3].integer(), -2);
    CHECK_NEAR(out[4].number(), 0.25, 1e-3);
    CHECK_EQ(out[5].integer(), 7);
    CHECK_NEAR(out[6].number(), 12.5, 1e-3);
    CHECK_EQ(out[7].str(), std::string_view("hé"));
    CHECK(!vm.take_mouse_grab().has_value());

    // Held key stays down; the press edge and the motion are gone.
    vm.key("g", true);
    vm.begin_frame(0.25, 8, 12.75);
    vm.run();
    out = vm.drain("out");
    CHECK_EQ(out[0].integer(), 1);
    CHECK_EQ(out[1].integer(), 0);
    CHECK_EQ(out[2].integer(), 0);
    CHECK_EQ(vm.take_mouse_grab(), std::optional<bool>(true));

    vm.key("space", false);
    vm.key("escape", true);
    vm.begin_frame(0.25, 9, 13.0);
    vm.run();
    CHECK_EQ(vm.drain("out")[0].integer(), 0);
    CHECK_EQ(vm.take_mouse_grab(), std::optional<bool>(false));

    CHECK_THROWS_CODE(vm.key("SPACE", true), PB_ERR_INVALID_ARG);
    CHECK(petal::Vm::is_canonical_key("leftbracket"));
    CHECK(!petal::Vm::is_canonical_key("lbracket"));
}

TEST(ui_draw_full_vocabulary) {
    petal::Vm vm;
    // A host extension command: an emitter into petal-ui's own buffer keeps
    // its place in the draw order and decodes as PB_DRAW_HOST.
    vm.emitter("draw_sprite", "draw_commands", "sprite");
    vm.set_text_metrics(0.5);
    vm.load_source(
        "draw_rect_outline(1, 2, 3, 4, 10, 20, 30, 200, 3)\n"
        "draw_rect_gradient({x: 0, y: 0, w: 50, h: 10}, #ff0000, #0000ff, 0.5)\n"
        "draw_circle_gradient(5, 6, 7, 1, 2, 3, 255, 4, 5, 6, 0)\n"
        "draw_ellipse(10, 11, 12, 13, 1, 1, 1)\n"
        "draw_ellipse_outline(10, 11, 12, 13, 1, 1, 1, 255, 2)\n"
        "fill_triangle(0, 0, 10, 0, 0, 10, 9, 9, 9)\n"
        "fill_polygon([[0, 0], [10, 0], [10, 10], [5, 3]], 1, 2, 3)\n"
        "fill_fan(5, 5, [[0, 0], [10, 0], [10, 10]], 1, 2, 3)\n"
        "draw_polyline([[0, 0], [5, 5], [9, 1]], 1, 2, 3, 255, 4)\n"
        "draw_sprite(\"hero\", 32, 48)\n"
        "clip(1, 2, 3, 4)\n"
        "clip_none()\n"
        "push_output(symbol(\"out\"), text_width(\"abcd\", 10))\n");
    vm.run();
    auto cmds = vm.drain_draw();
    REQUIRE(cmds.size() == 12);
    CHECK_EQ(cmds[0].kind, uint32_t(PB_DRAW_RECT_OUTLINE));
    CHECK_EQ(cmds[0].width, 3);
    CHECK_EQ(int(cmds[0].color.a), 200);
    CHECK_EQ(cmds[1].kind, uint32_t(PB_DRAW_RECT_GRADIENT));
    CHECK_EQ(cmds[1].w, 50);
    CHECK_EQ(int(cmds[1].color.r), 255);
    CHECK_EQ(int(cmds[1].color2.b), 255);
    CHECK_NEAR(double(cmds[1].angle), 0.5, 1e-3);
    CHECK_EQ(cmds[2].kind, uint32_t(PB_DRAW_CIRCLE_GRADIENT));
    CHECK_EQ(cmds[2].radius, 7);
    CHECK_EQ(int(cmds[2].color2.r), 4);
    CHECK_EQ(cmds[3].kind, uint32_t(PB_DRAW_ELLIPSE));
    CHECK_EQ(cmds[3].ry, 13);
    CHECK_EQ(cmds[4].kind, uint32_t(PB_DRAW_ELLIPSE_OUTLINE));
    CHECK_EQ(cmds[4].width, 2);
    CHECK_EQ(cmds[5].kind, uint32_t(PB_DRAW_TRIANGLE));
    CHECK_EQ(cmds[5].y3, 10);
    CHECK_EQ(cmds[6].kind, uint32_t(PB_DRAW_POLYGON));
    CHECK_EQ(cmds[6].point_count, size_t(4));
    CHECK_EQ(cmds[7].kind, uint32_t(PB_DRAW_FAN));
    CHECK_EQ(cmds[7].cx, 5);
    CHECK_EQ(cmds[7].point_count, size_t(3));
    CHECK_EQ(cmds[8].kind, uint32_t(PB_DRAW_POLYLINE));
    CHECK_EQ(cmds[8].width, 4);
    CHECK_EQ(cmds[9].kind, uint32_t(PB_DRAW_HOST));
    CHECK_EQ(std::string(cmds[9].text), std::string("sprite"));
    REQUIRE(cmds[9].data_count == 3);
    CHECK_EQ(std::string(cmds[9].data[0].str), std::string("hero"));
    CHECK_EQ(cmds[9].data[2].integer, 48);
    CHECK_EQ(cmds[10].kind, uint32_t(PB_DRAW_CLIP));
    CHECK_EQ(cmds[11].kind, uint32_t(PB_DRAW_CLIP_NONE));
    CHECK_NEAR(vm.drain("out")[0].number(), 20.0, 1e-3);  // 4 glyphs * 10px * 0.5
}

TEST(text_advance_table_measures_per_glyph) {
    petal::Vm vm;
    vm.set_text_metrics(0.5);
    // 'a' (97) is narrow, 'b' (98) wide; 'c' falls off the end of the table.
    std::vector<double> advances(99, 0.5);
    advances[97] = 0.2;
    advances[98] = 1.0;
    vm.set_text_advances(advances);
    vm.load_source("push_output(symbol(\"out\"), text_width(\"abc\", 10))\n");
    CHECK_EQ(run_out(vm), std::vector<double>{2.0 + 10.0 + 5.0});
    // An empty table is uniform again.
    vm.set_text_advances({});
    CHECK_EQ(run_out(vm), std::vector<double>{15.0});
}

TEST(seeded_random_is_reproducible) {
    auto sample = [] {
        petal::Vm vm;
        vm.set_seed(1234);
        vm.load_source("push_output(symbol(\"out\"), random(0, 1000000))\n");
        std::vector<double> v;
        for (int i = 0; i < 3; ++i) v.push_back(run_out(vm)[0]);
        return v;
    };
    auto a = sample(), b = sample();
    CHECK(a == b);
    CHECK(a[0] != a[1]);
}

// ─── Errors ─────────────────────────────────────────────────────────────────

TEST(compile_errors_are_structured) {
    petal::Vm vm;
    vm.load_source("push_output(symbol(\"out\"), 1)\n");
    petal::Error e = CHECK_THROWS_CODE(vm.load_source("let x = 1\nlet y = (2 +\n", "broken.ptl"), PB_ERR_COMPILE);
    CHECK_EQ(e.file(), std::string("broken.ptl"));
    CHECK(e.line() >= 2);
    CHECK(e.column() > 0);
    CHECK(!e.phase().empty());
    CHECK(!e.items().empty());

    // Compiler diagnostics carry spans too.
    petal::Error e2 = CHECK_THROWS_CODE(vm.load_source("let a = 1\nset a = 2\n", "sem.ptl"), PB_ERR_COMPILE);
    CHECK_EQ(e2.line(), 2u);
    CHECK_EQ(e2.phase(), std::string("compile"));

    // The previously loaded program is still there.
    CHECK_EQ(run_out(vm), std::vector<double>{1});
    const pb_error* last = pb_vm_last_error(vm.raw());
    CHECK(last == nullptr);
}

TEST(runtime_errors_are_reported) {
    petal::Vm vm;
    vm.load_source(
        "state frames = 0\n"
        "frames += 1\n"
        "push_output(symbol(\"out\"), frames)\n"
        "if frames == 2 then\n"
        "  let f = 3\n"
        "  f(1)\n"
        "end\n");
    CHECK_EQ(run_out(vm), std::vector<double>{1});
    petal::Error e = CHECK_THROWS_CODE(vm.run(), PB_ERR_RUNTIME);
    CHECK(std::string(e.what()).find("Cannot call int") != std::string::npos);
    CHECK_EQ(e.line(), 6u);
    CHECK_EQ(e.column(), 3u);
    CHECK_EQ(e.items()[0].message, std::string("Cannot call int"));
    // Output emitted before the error is still drainable.
    CHECK_EQ(vm.drain("out").size(), size_t(1));
    // The VM keeps going on later frames.
    CHECK_EQ(run_out(vm), std::vector<double>{3});
}

// ─── Modules, packages, implicit imports ───────────────────────────────────

TEST(registered_modules_and_implicit_imports) {
    petal::Vm vm;
    vm.register_module("engine", "export fn twice(x)\n  x * 2\nend\n");
    vm.add_implicit_import("engine");
    vm.register_module("util", "export fn inc(x)\n  x + 1\nend\n");
    vm.load_source(
        "import util\n"
        "push_output(symbol(\"out\"), twice(util.inc(20)))\n"
        "draw_rect(1, 2, 3, 4, 5, 6, 7)\n");  // `ui` natives still present
    CHECK_EQ(run_out(vm), std::vector<double>{42});
}

TEST(packages_register) {
    petal::Vm vm;
    std::string name = vm.add_package(std::string(PETAL_LIBS_DIR) + "/bloom");
    CHECK_EQ(name, std::string("bloom"));
    CHECK_THROWS_CODE(vm.add_package("/definitely/not/a/package"), PB_ERR_NOT_FOUND);
}

// ─── Hot reload ─────────────────────────────────────────────────────────────

TEST(hot_reload_preserves_state) {
    fs::path dir = scratch_dir("hot_reload");
    fs::path main = dir / "game.ptl";
    write_file(main,
               "state count = 0\n"
               "count += 1\n"
               "push_output(symbol(\"out\"), count * 1)\n");
    petal::Vm vm;
    vm.load_file(main.string());
    CHECK_EQ(run_out(vm), std::vector<double>{1});
    CHECK_EQ(run_out(vm), std::vector<double>{2});
    CHECK(!vm.sources_changed());

    // Edit a constant while "running".
    write_file(main,
               "state count = 0\n"
               "state extra = 5\n"
               "count += 1\n"
               "push_output(symbol(\"out\"), count * 100 + extra)\n");
    CHECK(vm.sources_changed());
    // state_preserved also counts the `ui` prelude's own state slots, so only
    // the drop count is exact.
    petal::ReloadResult r = vm.reload();
    CHECK(r.state_preserved >= 1u);
    CHECK_EQ(r.state_dropped, 0u);
    CHECK(!vm.sources_changed());
    CHECK_EQ(run_out(vm), std::vector<double>{305});

    // A broken edit keeps the old program running and reports where.
    write_file(main, "state count = 0\ncount += (\n");
    CHECK(vm.sources_changed());
    petal::Error e = CHECK_THROWS_CODE(vm.reload(), PB_ERR_COMPILE);
    CHECK_EQ(e.file(), main.string());
    CHECK(e.line() >= 2);
    CHECK(!vm.sources_changed());  // this version was looked at
    CHECK_EQ(run_out(vm), std::vector<double>{405});

    // Fixing it resumes with state intact; removed state is dropped.
    write_file(main, "state count = 0\ncount += 1\npush_output(symbol(\"out\"), count)\n");
    r = vm.reload();
    CHECK(r.state_preserved >= 1u);
    CHECK_EQ(r.state_dropped, 1u);  // `extra`
    CHECK_EQ(run_out(vm), std::vector<double>{5});
}

TEST(hot_reload_tracks_imported_modules) {
    fs::path dir = scratch_dir("hot_reload_imports");
    write_file(dir / "tuning.ptl", "export let SPEED = 2\n");
    write_file(dir / "main.ptl",
               "import tuning\n"
               "state pos = 0\n"
               "pos += tuning.SPEED\n"
               "push_output(symbol(\"out\"), pos)\n");
    petal::Vm vm;
    vm.load_file((dir / "main.ptl").string());
    auto files = vm.source_files();
    REQUIRE(files.size() == 2);
    CHECK_EQ(fs::path(files[0]).filename().string(), std::string("main.ptl"));
    CHECK_EQ(fs::path(files[1]).filename().string(), std::string("tuning.ptl"));

    CHECK_EQ(run_out(vm), std::vector<double>{2});
    CHECK(vm.changed_sources().empty());
    write_file(dir / "tuning.ptl", "export let SPEED = 10\n");
    CHECK(vm.sources_changed());
    // Only the edited module is named, with the path source_files() gives it.
    CHECK_EQ(vm.changed_sources(), std::vector<std::string>{files[1]});
    vm.reload();
    CHECK(vm.changed_sources().empty());
    CHECK_EQ(run_out(vm), std::vector<double>{12});

    // Both files edited: named in source_files() order.
    write_file(dir / "tuning.ptl", "export let SPEED = 1\n");
    write_file(dir / "main.ptl",
               "import tuning\n"
               "state pos = 0\n"
               "pos += tuning.SPEED * 2\n"
               "push_output(symbol(\"out\"), pos)\n");
    CHECK_EQ(vm.changed_sources(), files);
}

TEST(hot_reload_watches_new_imports_and_deletions) {
    fs::path dir = scratch_dir("hot_reload_watch");
    write_file(dir / "main.ptl", "push_output(symbol(\"out\"), 1)\n");
    write_file(dir / "extra.ptl", "export let BONUS = 5\n");
    petal::Vm vm;
    vm.load_file((dir / "main.ptl").string());
    CHECK_EQ(vm.source_files().size(), size_t(1));

    // The reloaded program imports a new module: it is watched from then on.
    write_file(dir / "main.ptl", "import extra\npush_output(symbol(\"out\"), extra.BONUS)\n");
    CHECK(vm.sources_changed());
    vm.reload();
    CHECK_EQ(vm.source_files().size(), size_t(2));
    CHECK_EQ(run_out(vm), std::vector<double>{5});
    CHECK(!vm.sources_changed());

    // Deleting a watched file counts as a change; the reload then fails as a
    // compile error and the old program keeps running.
    fs::remove(dir / "extra.ptl");
    CHECK(vm.sources_changed());
    auto changed = vm.changed_sources();
    REQUIRE(changed.size() == 1);
    CHECK_EQ(fs::path(changed[0]).filename().string(), std::string("extra.ptl"));
    CHECK_THROWS_CODE(vm.reload(), PB_ERR_COMPILE);
    CHECK(!vm.sources_changed());
    CHECK_EQ(run_out(vm), std::vector<double>{5});

    // Bringing it back is a change too.
    write_file(dir / "extra.ptl", "export let BONUS = 7\n");
    CHECK(vm.sources_changed());
    vm.reload();
    CHECK_EQ(run_out(vm), std::vector<double>{7});
}

TEST(reload_from_source) {
    petal::Vm vm;
    vm.native("host_scale", [](petal::Call& c) { c.result().value(c[0].number() * 10); });
    vm.load_source("state n = 1\nn += 1\npush_output(symbol(\"out\"), n)\n");
    CHECK_EQ(run_out(vm), std::vector<double>{2});
    petal::ReloadResult r = vm.reload_source("state n = 1\nn += 1\npush_output(symbol(\"out\"), host_scale(n))\n");
    CHECK(r.state_preserved >= 1u);
    CHECK_EQ(r.state_dropped, 0u);
    CHECK_EQ(run_out(vm), std::vector<double>{30});
    // pb_vm_reload needs a file-backed program.
    CHECK_THROWS_CODE(vm.reload(), PB_ERR_INVALID_ARG);
}

// ─── Input scenarios ────────────────────────────────────────────────────────

TEST(scenario_parses_and_reports) {
    petal::Scenario sc = petal::Scenario::from_json(R"({
        "size": [320, 200], "frames": 12,
        "events": [
            {"at": 2, "click": [40, 50]},
            {"at": 5, "key": "space"},
            {"at": 7, "text": "hi"}
        ]})");
    CHECK_EQ(sc.event_count(), size_t(6));  // click = move + down + up(next frame); key = down + up
    CHECK_EQ(sc.end_frame(), size_t(8));
    CHECK_EQ(sc.frames(), std::optional<size_t>(12));
    CHECK(sc.size() == (std::optional<std::pair<int32_t, int32_t>>(std::pair{320, 200})));
    petal::Scenario again = petal::Scenario::from_json(sc.to_json());
    CHECK_EQ(again.event_count(), sc.event_count());

    petal::Scenario bare = petal::Scenario::from_json(R"({"events": []})");
    CHECK(!bare.frames().has_value());
    CHECK(!bare.size().has_value());
    CHECK_EQ(bare.end_frame(), size_t(0));

    petal::Error bad_key =
        CHECK_THROWS_CODE(petal::Scenario::from_json(R"({"events": [{"at": 1, "key": "ArrowLeft"}]})"),
                          PB_ERR_INVALID_ARG);
    CHECK_CONTAINS(std::string(bad_key.what()), "canonical");
    CHECK_THROWS_CODE(petal::Scenario::from_json("{not json"), PB_ERR_INVALID_ARG);
    CHECK_THROWS_CODE(petal::Scenario::from_file("/definitely/not/here.json"), PB_ERR_IO);

    // A failed load keeps the previous contents and reports why (C API).
    pb_scenario* raw = pb_scenario_new();
    CHECK_EQ(pb_scenario_load_json(raw, R"({"events": [{"at": 0, "text": "x"}]})"), PB_OK);
    CHECK(pb_scenario_error(raw) == nullptr);
    CHECK_EQ(pb_scenario_load_json(raw, "[]"), PB_ERR_INVALID_ARG);
    CHECK(pb_scenario_error(raw) != nullptr);
    CHECK_EQ(pb_scenario_event_count(raw), size_t(1));
    pb_scenario_free(raw);
}

TEST(scenario_drives_the_ui) {
    fs::path dir = scratch_dir("scenario");
    write_file(dir / "clicks.json", R"({"size": [400, 300], "frames": 10, "events": [
        {"at": 1, "click": [50, 20]},
        {"at": 4, "click": [300, 200]},
        {"at": 6, "key": "space"},
        {"at": 8, "text": "ok"}
    ]})");
    petal::Scenario sc = petal::Scenario::from_file((dir / "clicks.json").string());
    petal::Vm vm;
    auto [w, h] = *sc.size();
    vm.set_dimensions(w, h);
    vm.load_source(R"petal(
state clicks = 0
state spaces = 0
state typed = ""
if button({x: 10, y: 10, w: 100, h: 30}, "Press") then clicks += 1 end
if key_pressed("space") then spaces += 1 end
typed = typed ++ text_input()
)petal");
    size_t applied = 0;
    for (size_t frame = 0; frame < *sc.frames(); ++frame) {
        applied += vm.apply_scenario(sc, frame);
        vm.begin_frame(1.0 / 60, int64_t(frame), frame / 60.0);
        vm.run();
    }
    CHECK_EQ(applied, sc.event_count());
    CHECK_EQ(vm.state("clicks").integer(), 1);  // the second click misses the button
    CHECK_EQ(vm.state("spaces").integer(), 1);
    CHECK_EQ(vm.state("typed").str(), std::string_view("ok"));
}

TEST(scenario_monkey_is_deterministic) {
    petal::Scenario a = petal::Scenario::monkey(7, 60, 400, 300);
    petal::Scenario b = petal::Scenario::monkey(7, 60, 400, 300);
    CHECK(a.event_count() > 0);
    CHECK_EQ(a.to_json(), b.to_json());
    CHECK(a.to_json() != petal::Scenario::monkey(8, 60, 400, 300).to_json());
    CHECK_EQ(a.frames(), std::optional<size_t>(60));

    // Replaying it runs a UI script without error.
    petal::Vm vm;
    vm.set_dimensions(400, 300);
    vm.load_source(R"petal(
state n = 0
if mouse_pressed(0) then n += 1 end
button({x: 0, y: 0, w: 200, h: 100}, "B")
)petal");
    for (size_t f = 0; f < 60; ++f) {
        vm.apply_scenario(a, f);
        vm.begin_frame(1.0 / 60, int64_t(f), f / 60.0);
        vm.run();
    }
    CHECK(vm.state("n").integer() > 0);
}

// ─── Main ───────────────────────────────────────────────────────────────────

int main(int argc, char** argv) {
    std::printf("%s\n", pb_version());
    petal_test::describe_exception = [](const std::exception& e) {
        if (const auto* pe = dynamic_cast<const petal::Error*>(&e))
            return std::string("unexpected petal::Error (") + pb_status_name(pe->code()) + "): " + e.what();
        return std::string("unexpected exception: ") + e.what();
    };
    return petal_test::run_tests(argc, argv);
}
