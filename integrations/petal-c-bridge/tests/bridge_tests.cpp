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
#include <tuple>
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
    write_file(dir / "main.ptl", "import io; push_output(symbol(\"out\"), 42)\n");
    petal::Vm vm;
    vm.load_file((dir / "main.ptl").string());
    CHECK_EQ(run_out(vm), std::vector<double>{42});
    CHECK_THROWS_CODE(vm.load_file((dir / "missing.ptl").string()), PB_ERR_IO);
}

TEST(state_persists_across_frames) {
    petal::Vm vm;
    vm.load_source(
        "import io; state n = 0\n"
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
    vm.load_source("import io; state n = 100\nn += 1\npush_output(symbol(\"out\"), n)\n");
    CHECK_EQ(run_out(vm), std::vector<double>{101});
}

TEST(memo_switch_keeps_output_and_state) {
    petal::Vm vm;
    vm.load_source(
        "import io; fn counter(step)\n"
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

TEST(static_rebuilds_wait_for_a_drag_and_can_be_flushed) {
    petal::Vm vm;
    vm.load_source(
        "import io; let ROWS = [[1, 2], [3, 4], [5, 6]]\n"
        "state n = 0\n"
        "n += 1\n"
        "push_output(symbol(\"out\"), ROWS[n % 1][1] * 100 + n)\n");
    // Every build counts as slow, and no pause is long enough.
    vm.set_static_debounce(0, 60000);
    CHECK_EQ(run_out(vm), std::vector<double>{201});
    // The first tick of a drag rebuilds at once.
    CHECK(vm.set_tunable("ROWS[0][1]", "{\"int\": 3}"));
    CHECK(vm.last_reload_json().find("\"rebuilt_names\":[\"ROWS\"]") != std::string::npos);
    CHECK_EQ(run_out(vm), std::vector<double>{302});
    // The second waits: the table keeps the value of the tick before.
    CHECK(vm.set_tunable("ROWS[0][1]", "{\"int\": 4}"));
    CHECK(vm.last_reload_json().find("\"deferred\":[\"ROWS\"]") != std::string::npos);
    CHECK_EQ(run_out(vm), std::vector<double>{303});
    // The host knows the drag ended.
    CHECK_EQ(vm.flush_static_rebuilds(), 1);
    CHECK_EQ(vm.flush_static_rebuilds(), 0);
    CHECK_EQ(run_out(vm), std::vector<double>{404});
    // Off: every tick rebuilds. And a reload that follows nothing reports
    // nothing about constants.
    vm.set_static_debounce(-1, 0);
    CHECK(vm.set_tunable("ROWS[0][1]", "{\"int\": 5}"));
    CHECK(vm.set_tunable("ROWS[0][1]", "{\"int\": 6}"));
    CHECK(vm.last_reload_json().find("\"deferred\":[]") != std::string::npos);
    CHECK_EQ(run_out(vm), std::vector<double>{605});
    vm.set_static_follow(false);
    CHECK(vm.set_tunable("ROWS[0][1]", "{\"int\": 7}"));
    CHECK(vm.last_reload_json().find("\"statics\"") == std::string::npos);
    CHECK_EQ(run_out(vm), std::vector<double>{706});
}

// A `const` built with a native that is not PB_FX_CONST loads and runs as a
// `let`; diagnostics_text() is where the host hears of it.
TEST(a_const_that_is_not_constant_is_reported_to_the_host) {
    for (bool is_const : {false, true}) {
        petal::Vm vm;
        vm.native("ticks", is_const ? petal::fx::Const : petal::fx::Pure, [](petal::Call& c) { c.result().value(int64_t(7)); });
        vm.load_source(
            "import io\n"
            "native fn ticks() -> int\n"
            "const T = ticks()\n"
            "push_output(symbol(\"out\"), T)\n");
        CHECK_EQ(run_out(vm), std::vector<double>{7});
        const std::string errors = vm.diagnostics_text();
        if (is_const) CHECK_EQ(errors, std::string());
        else CHECK(errors.find("`T` is declared `const` but is not constant") != std::string::npos);
    }
}

TEST(static_cache_switch_keeps_output_and_reports) {
    petal::Vm vm;
    vm.load_source(
        "import io; const ROWS = [[1, 2], [3, 4], [5, 6]]\n"
        "state n = 0\n"
        "n += 1\n"
        "push_output(symbol(\"out\"), ROWS[n % 3][1] + n)\n");
    CHECK(vm.static_cache());
    CHECK_EQ(run_out(vm), std::vector<double>{5});
    CHECK_EQ(run_out(vm), std::vector<double>{8});
    std::string report = vm.static_values_json();
    CHECK(report.find("\"enabled\":true") != std::string::npos);
    // The second run skipped what the first one built.
    CHECK(report.find("\"skips\":0,") == std::string::npos);
    CHECK(report.find("\"filled\":0,") == std::string::npos);
    CHECK(report.find("\"rejected\":[]") != std::string::npos);
    // Clearing rebuilds the table on the next run; switching the cache off
    // (and back on) changes nothing a program can see.
    vm.clear_static_values();
    CHECK_EQ(run_out(vm), std::vector<double>{5});
    vm.set_static_cache(false);
    CHECK(!vm.static_cache());
    CHECK_EQ(run_out(vm), std::vector<double>{8});
    CHECK(std::string(vm.static_values_json()).find("\"slots\":0") != std::string::npos);
    vm.set_static_cache(true);
    CHECK_EQ(run_out(vm), std::vector<double>{11});
    CHECK_EQ(run_out(vm), std::vector<double>{8});
}

TEST(profiler_reports_functions_and_natives) {
    petal::Vm vm;
    vm.load_source(
        "import io; fn spin(n)\n"
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

// Profiling can be on before anything is loaded, so the report covers the
// first run: the boot work a later start would miss.
TEST(profiler_covers_the_first_run_when_turned_on_before_the_load) {
    petal::Vm vm;
    vm.set_profiling(true);
    vm.load_source(
        "import io; fn boot(n)\n"
        "  let acc = 0.0\n"
        "  for i in range(0, n) do acc = acc + sqrt(float(i)) end\n"
        "  acc\n"
        "end\n"
        "state table = boot(200)\n"
        "push_output(symbol(\"out\"), table)\n");
    vm.run();
    std::string first = vm.profile_report(10);
    CHECK_CONTAINS(first, "boot");
    // The initializer does not run again, and the counts are cumulative.
    vm.run();
    CHECK_CONTAINS(vm.profile_report(10), "boot");
}

TEST(restart_starts_state_over_without_recompiling) {
    petal::Vm vm;
    CHECK_THROWS_CODE(vm.restart(), PB_ERR_NOT_LOADED);
    vm.load_source("import io; state n = 0\nn += 1\npush_output(symbol(\"out\"), n)\n");
    CHECK_EQ(run_out(vm), std::vector<double>{1});
    CHECK_EQ(run_out(vm), std::vector<double>{2});
    vm.restart();
    CHECK_EQ(run_out(vm), std::vector<double>{1});
}

TEST(buffers_are_cleared_each_run) {
    petal::Vm vm;
    vm.load_source("import io; push_output(symbol(\"out\"), 1)\n");
    vm.run();
    vm.run();  // the first run's value was never drained
    CHECK_EQ(vm.drain("out").size(), size_t(1));
    CHECK_EQ(vm.drain("out").size(), size_t(0));  // drained
}

// ─── Bindings ───────────────────────────────────────────────────────────────

TEST(bindings_reach_the_script) {
    petal::Vm vm;
    vm.load_source(
        "import io; fn b(name) binding(symbol(name)) end\n"
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
        "import io; import vec2; native fn host_up(...args)\nlet out = symbol(\"out\")\n"
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
        "import io; import vec2; enum Shape\n"
        "  Circle(radius),\n"
        "  Box(w, h),\n"
        "  Empty,\n"
        "end\n"
        "let out = symbol(\"out\")\n"
        "push_output(out, {\n"
        "  name: \"crate\",\n"
        "  pos: {x: 1.5, y: 2, z: -3},\n"
        "  tags: [\"wood\", \"heavy\"],\n"
        "  shape: Shape.Box(2, 3),\n"
        "  inner: [[1, 2], [], [Shape.Circle(0.5)]],\n"
        "  vel: vec2(1, -1),\n"
        "  nothing: nil,\n"
        "  on: false,\n"
        "})\n"
        "push_output(out, Shape.Circle(4))\n"
        "push_output(out, Shape.Empty)\n"
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
    CHECK_EQ(rec["shape"].tag(), std::string_view("Shape.Box"));
    CHECK_EQ(rec["shape"][1].integer(), 3);
    CHECK_EQ(rec["inner"][0][1].integer(), 2);
    CHECK(rec["inner"][1].is_list() && rec["inner"][1].empty());
    CHECK_EQ(rec["inner"][2][0].tag(), std::string_view("Shape.Circle"));
    CHECK_NEAR(rec["inner"][2][0][0].number(), 0.5, 1e-3);
    CHECK(rec["vel"].is_vec2());
    CHECK(rec.has("nothing") && rec["nothing"].is_nil());
    CHECK(!rec.has("absent"));
    CHECK(rec["on"].is_bool() && !rec["on"].boolean());
    CHECK(rec["absent"]["deeper"][3].is_nil());  // chained misses are safe

    CHECK_EQ(out[1].tag(), std::string_view("Shape.Circle"));
    CHECK_EQ(out[1][0].integer(), 4);
    CHECK_EQ(out[2].tag(), std::string_view("Shape.Empty"));
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
        "import io; push_output(symbol(\"a\"), {k: \"first\"})\n"
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
        "import io; native fn host_add(...args); native fn raycast(...args); native fn names(...args); native fn nothing()\nlet out = symbol(\"out\")\n"
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
    vm.load_source("native fn explode()\nstate n = 0\nn += 1\nexplode()\n");
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
    const char* src = "import io; native fn who()\npush_output(symbol(\"out\"), who())\n";
    a.load_source(src);
    b.load_source(src);
    a.run();
    b.run();
    CHECK_EQ(a.drain("out")[0].str(), std::string_view("a"));
    CHECK_EQ(b.drain("out")[0].str(), std::string_view("b"));
}

// PB_FX_CONST is a registration flag like the others: a native declared a
// constant function registers, is called, and returns its value. (What Petal
// may do with the declaration - evaluate a top-level table once - is the
// language's business; how often the callback runs is deliberately not
// asserted here.)
TEST(const_native_registers_and_runs) {
    petal::Vm vm;
    vm.native("tile_code", petal::fx::Const, [](petal::Call& c) { c.result().value(c[0].number() * 2); });
    CHECK_EQ(pb_vm_register_native(vm.raw(), "k2", [](pb_call*, void*) -> int { return 0; }, nullptr, nullptr,
                                   PB_FX_CONST | PB_FX_PENDING_ALLOW),
             PB_OK);
    vm.load_source("import io; native fn tile_code(n)\nlet T = [tile_code(1), tile_code(2)]\n"
                   "push_output(symbol(\"out\"), T[1])\n");
    vm.run();
    CHECK_EQ(vm.drain("out")[0].number(), 4.0);
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
    vm.load_source("native fn sneaky()\nsneaky()\n");
    vm.run();
    CHECK_EQ(inner, PB_ERR_REENTRANT);
}

TEST(emitters_push_tagged_commands) {
    petal::Vm vm;
    vm.emitter("spawn", "scene");
    vm.emitter("light", "scene", "point_light");
    vm.emitter("sfx", "audio");
    vm.load_source(
        "import io; native fn spawn(...args); native fn light(...args); native fn sfx(...args)\nlet r = spawn(\"cube\", {x: 1, y: 2, z: 3}, 0.5)\n"
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

TEST(propose_edit_drags_through_host_natives) {
    // Direct manipulation on the host's VM: a drag of what to_screen(px)
    // emitted finds `px`, applies it, and the reload draws it there. The
    // escaping native never fires in the speculative runs; the contained one may.
    fs::path dir = scratch_dir("propose_edit");
    fs::path file = dir / "scene.ptl";
    write_file(file,
               "import io\nnative fn to_screen(x)\nnative fn play_sound()\n"
               "config let px = 10\nplay_sound()\n"
               "push_output(symbol(\"out\"), to_screen(px))\n");
    petal::Vm vm;
    int sounds = 0, screens = 0;
    vm.native("to_screen", petal::fx::Pure, [&](petal::Call& c) {
        ++screens;
        c.result().value(c[0].number() * 2 + 100);
    });
    vm.native("play_sound", petal::fx::Effect, [&](petal::Call&) { ++sounds; });
    vm.load_file(file.string());
    CHECK_EQ(run_out(vm), std::vector<double>{120});
    CHECK_EQ(sounds, 1);

    std::string r = vm.propose_edit(R"({"channel": "out", "emit": 0, "value": true, "to": 150})");
    CHECK(r.find("\"verified\":true") != std::string::npos);
    CHECK(r.find("\"new_text\":\"25\"") != std::string::npos);
    CHECK_EQ(sounds, 1);
    CHECK(screens > 3);

    r = vm.propose_edit(R"({"channel": "out", "emit": 0, "value": true, "to": 150, "apply": true, "pick": 1})");
    CHECK(r.find("\"applied\"") != std::string::npos);
    REQUIRE(vm.sources_changed());
    vm.reload();
    CHECK_EQ(run_out(vm), std::vector<double>{150});

    // A failed proposal is the error object, not an exception.
    r = vm.propose_edit(R"({"channel": "out", "emit": 3, "value": true, "to": 1})");
    CHECK(r.find("\"error\":true") != std::string::npos);
    CHECK_EQ(pb_vm_last_error(vm.raw())->code, PB_ERR_RUNTIME);
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
        "import io; native fn e2999(...args); native fn c0(); native fn c2998()\ne2999(1)\n"
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
        CHECK_EQ(pb_vm_load_source(vm, "import io; native fn answer()\npush_output(symbol(\"out\"), answer())\n", nullptr), PB_OK);
        CHECK_EQ(pb_vm_run(vm), PB_OK);
        pb_reload_result r;
        CHECK_EQ(pb_vm_reload_source(vm, "import io; native fn answer()\npush_output(symbol(\"out\"), answer() + 1)\n", &r), PB_OK);
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

// A replayed scope reaches the host as one entry naming a segment it was
// given before (petal_bridge.h, "Segmented output buffers").
// ─── Option plans ───────────────────────────────────────────────────────────

TEST(a_plan_reads_options_by_position_and_names_an_unknown_field) {
    petal::Vm vm;
    vm.emitter("body", "scene");
    vm.load_source(
        "native fn body(id, opts)\n"
        "for i in range(0, 3) do body(i, {mass: 2.0, pos: i, name: \"b{i}\"}) end\n"
        "body(9, {mass: 1.0, colour: 3})\n"
        "body(10, nil)\nbody(11, 5)\n");
    vm.run();
    petal::Values cmds = vm.drain("scene");
    REQUIRE(cmds.size() == 6);
    petal::Plan plan("shape, mass, pos, name");
    CHECK_EQ(plan.size(), (size_t)4);
    CHECK_EQ(plan.name(1), std::string_view("mass"));
    CHECK_EQ(plan.index("pos"), (size_t)2);
    CHECK_EQ(plan.index("nope"), (size_t)4);
    const uint64_t shape = cmds[0][1].shape();
    CHECK(shape != 0);
    for (size_t i = 0; i < 3; ++i) {
        petal::Value opts = cmds[i][1];
        CHECK_EQ(opts.shape(), shape);  // one literal, one shape
        auto o = plan.bind(opts);
        CHECK(o.ok());
        CHECK(o[0].is_nil());
        CHECK_EQ(o[1].number(), 2.0);
        CHECK_EQ(o[2].integer(), (int64_t)i);
        CHECK_EQ(o[3].string(), "b" + std::to_string(i));
        CHECK_EQ(o[3].key(), std::string_view("name"));
    }
    auto odd = plan.bind(cmds[3][1]);
    CHECK(!odd.ok());
    CHECK_EQ(odd.unknown().key(), std::string_view("colour"));
    CHECK_EQ(odd[1].number(), 1.0);  // the known options are still bound
    auto nil = plan.bind(cmds[4][1]);
    CHECK(nil.ok());
    CHECK(nil[1].is_nil());
    auto bad = plan.bind(cmds[5][1]);
    CHECK(!bad.ok());
    CHECK(bad.unknown().is_nil());
    CHECK_THROWS_CODE(petal::Plan("a, a"), PB_ERR_INVALID_ARG);
    CHECK(pb_plan_new("a,,b") == nullptr);
}

TEST(a_snapshot_reports_which_options_changed) {
    petal::Vm vm;
    vm.emitter("body", "scene");
    vm.load_source(
        "import vec2\nnative fn body(id, opts)\n"
        "state t = 0\nt += 1\n"
        "let p = if t < 3 then vec3(0, 0, 0) else vec3(1, 2, float(t)) end\n"
        "let mass = if t < 4 then 2.0 else 3.0 end\n"
        "body(1, {shape: \"box\", mass: mass, pos: p, mat: {color: #ff8800}})\n");
    petal::Plan plan("shape, mass, pos, mat");
    petal::Snapshot snap;
    std::vector<std::vector<size_t>> seen;
    for (int f = 0; f < 5; ++f) {
        vm.run();
        petal::Values cmds = vm.drain("scene");
        REQUIRE(cmds.size() == 1);
        auto o = plan.bind(cmds[0][1]);
        REQUIRE(o.ok());
        auto changed = snap.update(o);
        std::vector<size_t> bits(changed.begin(), changed.end());
        CHECK_EQ(changed.any(), !bits.empty());
        seen.push_back(bits);
    }
    CHECK_EQ(seen[0], (std::vector<size_t>{0, 1, 2, 3}));  // a new snapshot: everything
    CHECK(seen[1].empty());                                 // the same values, rebuilt
    CHECK_EQ(seen[2], (std::vector<size_t>{2}));            // pos moved
    CHECK_EQ(seen[3], (std::vector<size_t>{1, 2}));         // mass and pos
    CHECK_EQ(seen[4], (std::vector<size_t>{2}));
    snap.clear();
    vm.run();
    petal::Values cmds = vm.drain("scene");
    auto again = snap.update(plan.bind(cmds[0][1]));
    CHECK(again[0] && again[3]);
}

TEST(a_templated_record_carries_its_template_and_a_snapshot_compares_its_holes) {
    auto run = [](bool templates) {
        petal::Vm vm;
        vm.set_record_templates(templates);
        vm.emitter("body", "scene");
        vm.load_source(
            "import vec2\nnative fn body(id, opts)\n"
            "state t = 0\nt += 1\n"
            "fn ship(i, t)\n"
            "  body(i, {shape: \"box\", size: 2.0, mat: {color: #ff8800}, pos: vec3(float(i), 0, float(t))})\n"
            "end\n"
            "for i in range(0, 2) do ship(i, t) end\n");
        petal::Plan plan("shape, size, mat, pos");
        petal::Snapshot snaps[2];
        std::vector<std::vector<size_t>> seen;
        std::vector<uint64_t> ids;
        std::vector<std::string> dumps;
        for (int f = 0; f < 3; ++f) {
            vm.run();
            petal::Values cmds = vm.drain("scene");
            REQUIRE(cmds.size() == 2);
            for (size_t k = 0; k < 2; ++k) {
                petal::Value opts = cmds[k][1];
                auto o = plan.bind(opts);
                REQUIRE(o.ok());
                auto changed = snaps[k].update(opts, o);
                seen.emplace_back(changed.begin(), changed.end());
                ids.push_back(opts.template_id());
                dumps.push_back(std::string(o[0].str()) + "/" + std::to_string(o[1].number(0)) + "/" +
                                std::to_string(o[2]["color"].integer()) + "/" + std::to_string(o[3].z()));
            }
        }
        return std::make_tuple(seen, ids, dumps);
    };
    auto [seen, ids, dumps] = run(true);
    auto [seen_off, ids_off, dumps_off] = run(false);
    CHECK(dumps == dumps_off);  // the same values either way
    CHECK(seen == seen_off);    // and the same changes
    CHECK_EQ(seen[0], (std::vector<size_t>{0, 1, 2, 3}));
    CHECK_EQ(seen[2], (std::vector<size_t>{3}));  // only pos, the hole
    for (uint64_t id : ids) CHECK_EQ(id, ids[0]);
    CHECK(ids[0] != 0);
    for (uint64_t id : ids_off) CHECK_EQ(id, uint64_t(0));
}

TEST(host_built_return_records_share_a_shape) {
    petal::Vm vm;
    vm.native("state_of", [](petal::Call& c) {
        c.result().map([&](petal::BuilderRef m) {
            m.field("id", c[0].integer());
            m.key("pos").vec3(1, 2, 3);
            m.field("sleeping", false);
        });
    });
    vm.emitter("out", "out");
    vm.load_source("native fn state_of(id)\nnative fn out(v)\nfor i in range(0, 3) do out(state_of(i)) end\n");
    vm.run();
    petal::Values got = vm.drain("out");
    REQUIRE(got.size() == 3);
    const uint64_t shape = got[0][0].shape();
    CHECK(shape != 0);
    CHECK_EQ(got[1][0].shape(), shape);
    CHECK_EQ(got[2][0].shape(), shape);
    CHECK_EQ(got[2][0]["id"].integer(), (int64_t)2);
}

TEST(commands_reissue_queries_follow_their_class_and_declares_replay) {
    petal::Vm vm;
    std::vector<int64_t> kicks;
    int64_t physics = 1;
    int64_t ray_calls = 0;
    vm.native("kick", petal::fx::Command, [&](petal::Call& c) { kicks.push_back(c[0].integer()); });
    vm.native("ray", petal::fx::Query, [&](petal::Call& c) {
        ++ray_calls;
        c.result().value(physics * 10);
    });
    vm.emitter_fx("decl", "decls", petal::fx::Declare);
    vm.load_source(
        "import io\n"
        "native fn kick(id)\nnative fn ray()\nnative fn decl(id, x)\n"
        "fn pushers()\n  for i in range(0, 3) do kick(i) end\n  push_output(symbol(\"out\"), 1)\n  nil\nend\n"
        "fn aim()\n  push_output(symbol(\"out\"), ray())\n  nil\nend\n"
        "fn crates()\n  for i in range(0, 4) do decl(i, float(i)) end\n  nil\nend\n"
        "fn spin()\n  print(\"spin\")\nend\n"
        "pushers()\naim()\ncrates()\n");
    vm.set_memo(true);
    CHECK_EQ(run_out(vm), (std::vector<double>{1, 10}));
    CHECK_EQ(vm.drain("decls").size(), (size_t)4);
    // Replayed: the commands are issued again, the query is not re-asked,
    // and the declarations come back.
    kicks.clear();
    const int64_t asked = ray_calls;
    CHECK_EQ(run_out(vm), (std::vector<double>{1, 10}));
    CHECK_EQ(kicks, (std::vector<int64_t>{0, 1, 2}));
    CHECK_EQ(ray_calls, asked);
    CHECK_EQ(vm.drain("decls").size(), (size_t)4);
    // Host data in general does not invalidate a PHYSICS read; a step does.
    vm.note_host_data_changed();
    physics = 2;
    CHECK_EQ(run_out(vm), (std::vector<double>{1, 10}));
    vm.note_class_changed(petal::fx::ReadsPhysics);
    CHECK_EQ(run_out(vm), (std::vector<double>{1, 20}));
    const std::string report = vm.dep_classes_report();
    CHECK(report.find("aim: may PHYSICS") != std::string::npos);
    CHECK(report.find("never replays: spin: calls `print`") != std::string::npos);
}

// A command that changes a revisioned class during the run says so
// (pb_call_note_class_changed): a scope that read the class before the change
// is not replayed with the old answer, in this run or the next, while one
// that read it after a run in which nothing changed still replays.
TEST(a_command_that_changes_a_class_in_the_run_invalidates_its_readers) {
    for (const bool memo : {false, true}) {
        petal::Vm vm;
        double pos = 0;
        int64_t asked = 0;
        vm.native("move", petal::fx::Command, [&](petal::Call& c) {
            const double x = c[0].number();
            if (x == pos) return;
            pos = x;
            CHECK(c.note_class_changed(petal::fx::ReadsPhysics));
        });
        vm.native("where", petal::fx::Query, [&](petal::Call& c) {
            ++asked;
            c.result().value(pos);
        });
        vm.load_source(
            "import io\n"
            "native fn move(x)\nnative fn where()\n"
            "state n = 0\n"
            "n += 1\n"
            "fn show(tag)\n  push_output(symbol(\"out\"), where())\n  nil\nend\n"
            "show(1)\n"
            "move(float(min(n, 3)))\n"
            "show(2)\n");
        vm.set_memo(memo);
        for (int f = 1; f <= 6; ++f) {
            const int64_t before = asked;
            const double moved = double(std::min(f, 3));
            // The first show sees the last frame's move, the second this one's.
            CHECK_EQ(run_out(vm), (std::vector<double>{double(std::min(f - 1, 3)), moved}));
            // Once nothing moves (from frame 5 on, both records were made
            // after the last change) both shows replay.
            if (memo && f >= 5) CHECK_EQ(asked, before);
            if (!memo) CHECK_EQ(asked, before + 2);
        }
    }
}

// The same for host data in general (pb_call_note_class_changed with
// PB_FX_READS_HOST_DATA): an existence check such as `mesh_exists` is a read,
// not an effect, so a helper that only checks replays; a create later in the
// run that changes its answer bumps host data at once, so a scope that
// checked before it is not replayed with the old answer.
TEST(a_native_that_changes_host_data_in_the_run_invalidates_its_readers) {
    for (const bool memo : {false, true}) {
        petal::Vm vm;
        int64_t made = 0, asked = 0;
        vm.native("make", petal::fx::Effect, [&](petal::Call& c) {
            ++made;
            CHECK(c.note_class_changed(petal::fx::ReadsHostData));
        });
        vm.native("count", petal::fx::ReadsHostData | petal::fx::Contained, [&](petal::Call& c) {
            ++asked;
            c.result().value(made);
        });
        vm.load_source(
            "import io\n"
            "native fn make()\nnative fn count()\n"
            "state n = 0\n"
            "n += 1\n"
            "fn show(tag)\n  push_output(symbol(\"out\"), float(count()))\n  nil\nend\n"
            "show(1)\n"
            "if n == 2 || n == 3 then make() end\n"
            "show(2)\n");
        vm.set_memo(memo);
        for (int f = 1; f <= 6; ++f) {
            const int64_t before = asked;
            const double now = double(f >= 3 ? 2 : f >= 2 ? 1 : 0);
            const double then = double(f >= 4 ? 2 : f >= 3 ? 1 : 0);
            CHECK_EQ(run_out(vm), (std::vector<double>{then, now}));
            // From frame 5 on nothing is made and both records are newer
            // than the last change: both shows replay.
            if (memo && f >= 5) CHECK_EQ(asked, before);
            if (!memo) CHECK_EQ(asked, before + 2);
        }
    }
}

// A class whose revision the host SETS to a fingerprint of its state
// (PB_FX_READS_CAMERA, pb_vm_set_class_revision, pb_call_set_class_revision):
// an engine's frame camera, reset before every run and set again by a
// `look` command. A scope that sets the camera and then reads it replays
// while it starts from the same camera (Petal checks the read from the
// revision before the command, through the change it recorded), and a reader
// after it replays while the camera is the one of the run before; the
// command is re-issued by every replay.
TEST(a_camera_class_set_by_fingerprint_replays_while_the_camera_holds) {
    const auto fp = [](double cam) { return uint64_t(cam * 1000.0) * 0x9e3779b97f4a7c15ull + 7; };
    const double inputs[] = {1, 1, 2, 2, 1, 1};
    for (const bool memo : {false, true}) {
        petal::Vm vm;
        double cam = 0, input = 0;
        int64_t asked = 0, looks = 0;
        vm.native("look", petal::fx::Command, [&](petal::Call& c) {
            ++looks;
            cam = c[0].number();
            c.set_class_revision(petal::fx::ReadsCamera, fp(cam));
        });
        vm.native("proj", petal::fx::ReadsCamera, [&](petal::Call& c) {
            ++asked;
            c.result().value(cam * 10 + c[0].number());
        });
        vm.native("inp", petal::fx::Probe | petal::fx::ReadsPointer, [&](petal::Call& c) { c.result().value(input); });
        vm.load_source(
            "import io\n"
            "native fn look(x)\nnative fn proj(p)\nnative fn inp()\n"
            "fn shot(x, p)\n  look(x)\n  push_output(symbol(\"out\"), proj(p))\n  nil\nend\n"
            "fn hud(p)\n  push_output(symbol(\"out\"), proj(p))\n  nil\nend\n"
            "shot(inp(), 1.0)\n"
            "hud(2.0)\n");
        vm.set_memo(memo);
        for (int f = 0; f < 6; ++f) {
            input = inputs[f];
            cam = 0;   // the frame's camera starts from the default
            vm.set_class_revision(petal::fx::ReadsCamera, fp(cam));
            const int64_t before = asked;
            CHECK_EQ(run_out(vm), (std::vector<double>{input * 10 + 1, input * 10 + 2}));
            CHECK_EQ(looks, f + 1);   // issued or re-issued every frame
            // A frame with the camera of the frame before replays both.
            const bool same = f > 0 && inputs[f] == inputs[f - 1];
            CHECK_EQ(asked - before, (memo && same) ? 0 : 2);
        }
    }
}

TEST(implicit_keys_reach_keyed_emitters_and_callbacks) {
    petal::Vm vm;
    std::vector<std::optional<uint64_t>> lights, kicks;
    vm.emitter_fx("decl", "decls", petal::fx::Declare | petal::fx::Keyed);
    vm.native("light", petal::fx::Effect, [&](petal::Call& c) { lights.push_back(c.key()); });
    vm.native("kick", petal::fx::Command, [&](petal::Call& c) { kicks.push_back(c.key()); });
    vm.load_source(
        "native fn decl(v)\nnative fn light(v)\nnative fn kick(v)\n"
        "fn crate_at(i)\n  decl(i)\n  nil\nend\n"
        "fn pushers()\n  for i in range(0, 2) do kick(i) end\n  nil\nend\n"
        "for i in range(0, 3) do crate_at(i) end\n"
        "decl(99)\nlight(1)\nlight(2)\n"
        "[1, 2].forEach(fn(x) decl(x + 10) end)\n"
        "pushers()\n");
    vm.set_memo(true);
    auto frame = [&]() {
        lights.clear();
        kicks.clear();
        vm.run();
        std::vector<std::optional<uint64_t>> keys;
        petal::Values decls = vm.drain("decls");
        for (size_t i = 0; i < decls.size(); ++i) {
            CHECK_EQ(decls[i].tag(), std::string_view("decl"));
            REQUIRE(decls[i].size() == 2);
            if (decls[i][0].is_nil()) {
                keys.push_back(std::nullopt);
            } else {
                REQUIRE(decls[i][0].is_int());
                keys.push_back((uint64_t)decls[i][0].integer());
            }
        }
        return keys;
    };
    const auto first = frame();
    REQUIRE(first.size() == 6);
    // Three loop iterations and a top-level call: four keys, all different;
    // the forEach callback's calls have none.
    for (size_t i = 0; i < 4; ++i) {
        REQUIRE(first[i].has_value());
        for (size_t j = 0; j < i; ++j) CHECK(*first[i] != *first[j]);
    }
    CHECK(!first[4].has_value() && !first[5].has_value());
    REQUIRE(lights.size() == 2 && lights[0] && lights[1]);
    CHECK(*lights[0] != *lights[1]);
    REQUIRE(kicks.size() == 2 && kicks[0] && kicks[1]);
    const auto first_lights = lights, first_kicks = kicks;
    // The next frame replays `crate_at` and `pushers`: the same keys come
    // back with the declarations, and the re-issued commands get theirs.
    CHECK(frame() == first);
    CHECK(lights == first_lights);
    CHECK(kicks == first_kicks);
}

// A checked emitter (pb_vm_register_checked_emitter) runs its callback in
// the call, so it can check its arguments, then emits. A memoized scope
// that called it is replayed like one that called an emitter: the pushes
// come back and the callback is not called again.
TEST(a_checked_emitter_checks_in_the_call_and_replays_without_it) {
    petal::Vm vm;
    int64_t checked = 0, tokens = 0;
    vm.checked_emitter("box", "scene", petal::fx::Declare, [&](petal::Call& c) {
        ++checked;
        if (!c[0].is_number()) throw std::runtime_error("box: size must be a number");
    });
    // The result form: push what the callback made (a token), nothing for nil.
    vm.checked_emitter("mark", "scene", petal::fx::EmitResult, [&](petal::Call& c) {
        ++checked;
        if (c[0].number() > 0) c.result().value(++tokens * 100);
    }, "token");
    vm.checked_emitter("pin", "keyed", petal::fx::Declare | petal::fx::Keyed | petal::fx::EmitResult,
                       [&](petal::Call& c) { c.result().value(c[0].number() * 2); });
    vm.load_source(
        "native fn box(size, name)\nnative fn mark(n)\nnative fn pin(x)\n"
        "fn city(n)\n  for i in range(0, n) do box(float(i), \"b\") end\n  mark(1)\n  mark(0)\n  pin(4.0)\n  nil\nend\n"
        "city(3)\n");
    vm.set_memo(true);
    vm.run();
    petal::Values scene = vm.drain("scene");
    REQUIRE(scene.size() == 4);
    CHECK_EQ(scene[0].tag(), std::string_view("box"));
    REQUIRE(scene[2].size() == 2);
    CHECK_EQ(scene[2][0].number(), 2.0);
    CHECK_EQ(scene[2][1].str(), std::string_view("b"));
    CHECK_EQ(scene[3].tag(), std::string_view("token"));
    REQUIRE(scene[3].size() == 1);
    CHECK_EQ(scene[3][0].integer(), (int64_t)100);
    petal::Values keyed = vm.drain("keyed");
    REQUIRE(keyed.size() == 1 && keyed[0].size() == 2);
    CHECK(keyed[0][0].is_int());
    CHECK_EQ(keyed[0][1].number(), 8.0);
    CHECK_EQ(checked, (int64_t)5);

    // Replayed: the same pushes, the callbacks not called again.
    vm.run();
    scene = vm.drain("scene");
    REQUIRE(scene.size() == 4);
    CHECK_EQ(scene[1][0].number(), 1.0);
    CHECK_EQ(scene[3][0].integer(), (int64_t)100);
    CHECK_EQ(vm.drain("keyed").size(), (size_t)1);
    CHECK_EQ(checked, (int64_t)5);
    CHECK_EQ(tokens, (int64_t)1);

    // With memo off every call runs its callback.
    vm.set_memo(false);
    vm.run();
    CHECK_EQ(vm.drain("scene").size(), (size_t)4);
    CHECK_EQ(checked, (int64_t)10);

    // Flags that would make the call more than an emit are refused.
    CHECK_THROWS_CODE(vm.checked_emitter("bad", "scene", petal::fx::Command, [](petal::Call&) {}), PB_ERR_INVALID_ARG);
    CHECK_THROWS_CODE(vm.checked_emitter("bad", "scene", petal::fx::Effect, [](petal::Call&) {}), PB_ERR_INVALID_ARG);
}

// The callback's error is the script call's: it names the line that called
// the native, inside a helper, not a wrapper in a package.
TEST(a_checked_emitter_error_names_the_calling_line) {
    petal::Vm vm;
    vm.checked_emitter("box", "scene", petal::fx::Declare, [](petal::Call& c) {
        if (!c[0].is_number()) throw std::runtime_error("box: size must be a number");
    });
    vm.register_package("shapes", {{"petal.toml", "[package]\nname = \"shapes\"\nversion = \"1.0.0\"\n"},
                                   {"shapes", "pub native fn box(size)\n"}});
    vm.load_source(
        "import shapes\n"
        "fn wall()\n"
        "  box(1.0)\n"
        "  box(\"big\")\n"
        "  nil\n"
        "end\n"
        "wall()\n");
    petal::Error e = CHECK_THROWS_CODE(vm.run(), PB_ERR_RUNTIME);
    CHECK_CONTAINS(std::string(e.what()), "size must be a number");
    CHECK_EQ(e.line(), 4u);
    CHECK_EQ(e.column(), 3u);
    // The push made before the error is still drainable; the failed call made none.
    CHECK_EQ(vm.drain("scene").size(), (size_t)1);
}

// A watched counter (pb_vm_watch_class) revises its class without the host
// noting it: a command or effect that moves the counter in its call is a
// change in the run, and a move between runs is noted before the next run.
// The shape of Cheesecake's physics and asset reads (retained plan A3/A7):
// two readers around a native that moves the world; memo on must equal memo
// off, and with memo on a reader replays once nothing moves.
TEST(a_watched_counter_revises_its_class_in_the_call_and_between_runs) {
    struct World {
        double pos = 0;
        uint64_t revision = 0;
        uint64_t assets = 0;
    };
    const auto frames = [](bool memo, std::vector<int64_t>* asked_per_frame) {
        petal::Vm vm;
        World w;
        int64_t asked = 0;
        vm.watch_class(petal::fx::ReadsPhysics, &w.revision);
        vm.watch_class(petal::fx::ReadsHostData, &w.assets);
        vm.native("move", petal::fx::Command, [&](petal::Call& c) {
            const double x = c[0].number();
            if (x == w.pos) return;
            w.pos = x;
            ++w.revision;   // no note_class_changed: the watch sees it
        });
        vm.native("make", petal::fx::Effect, [&](petal::Call&) { ++w.assets; });
        vm.native("where", petal::fx::Query, [&](petal::Call& c) {
            ++asked;
            c.result().value(w.pos);
        });
        vm.native("count", petal::fx::ReadsHostData | petal::fx::Contained, [&](petal::Call& c) {
            ++asked;
            c.result().value(double(w.assets));
        });
        vm.load_source(
            "import io\n"
            "native fn move(x)\nnative fn make()\nnative fn where()\nnative fn count()\n"
            "state n = 0\n"
            "n += 1\n"
            "fn show(tag)\n  push_output(symbol(\"out\"), where())\n  push_output(symbol(\"out\"), count())\n  nil\nend\n"
            "show(1)\n"
            "move(float(min(n, 3)))\n"
            "if n == 2 || n == 3 then make() end\n"
            "show(2)\n");
        vm.set_memo(memo);
        std::vector<std::vector<double>> out;
        for (int f = 1; f <= 8; ++f) {
            // A physics step between runs (frame 6), and an asset made by the
            // host outside any call (frame 7).
            if (f == 6) { w.pos += 10; ++w.revision; }
            if (f == 7) ++w.assets;
            const int64_t before = asked;
            out.push_back(run_out(vm));
            if (asked_per_frame) asked_per_frame->push_back(asked - before);
        }
        return out;
    };
    std::vector<int64_t> asked_on;
    const auto off = frames(false, nullptr);
    const auto on = frames(true, &asked_on);
    CHECK(on == off);
    // What memo off sees: the first show reads last frame's world, the second this one's.
    REQUIRE(off.size() == 8);
    CHECK_EQ(off[1], (std::vector<double>{1, 0, 2, 1}));
    CHECK_EQ(off[5], (std::vector<double>{13, 2, 3, 2}));   // stepped, then moved back
    CHECK_EQ(off[6], (std::vector<double>{3, 3, 3, 3}));    // an asset made between runs
    // Frame 5 moves nothing and frame 8 moves nothing either: both shows replay.
    CHECK_EQ(asked_on[4], (int64_t)0);
    CHECK_EQ(asked_on[7], (int64_t)0);
    // The frames after a move between runs re-ask.
    CHECK(asked_on[5] > 0);
    CHECK(asked_on[6] > 0);

    // Only watchable classes; a NULL counter unwatches.
    petal::Vm vm;
    uint64_t c = 0;
    CHECK_THROWS_CODE(vm.watch_class(petal::fx::ReadsPointer, &c), PB_ERR_INVALID_ARG);
    vm.watch_class(petal::fx::ReadsNet, &c);
    vm.watch_class(petal::fx::ReadsNet, nullptr);
}

TEST(segmented_buffer_hands_a_replayed_scope_over_as_one_entry) {
    pb_vm* vm = pb_vm_create();
    CHECK_EQ(pb_vm_set_segmented(vm, "out", true), PB_OK);
    CHECK_EQ(pb_vm_load_source(vm,
                               "import io\n"
                               "fn wall(n)\n  for i in range(0, n) do push_output(symbol(\"out\"), i) end\n  nil\nend\n"
                               "wall(100)\npush_output(symbol(\"out\"), \"tail\")\n",
                               nullptr),
             PB_OK);
    CHECK_EQ(pb_vm_run(vm), PB_OK);
    pb_segments seg;
    CHECK_EQ(pb_vm_drain_segments(vm, "out", &seg), PB_OK);
    REQUIRE(seg.count == 2);
    REQUIRE(seg.defined_count == 1);
    const uint64_t wall = seg.defined[0].serial;
    CHECK(wall != 0);
    CHECK_EQ(seg.entries[0].segment, wall);
    CHECK(seg.entries[0].value == nullptr);
    CHECK_EQ(seg.entries[1].segment, (uint64_t)0);
    CHECK_EQ(std::string(seg.entries[1].value->str), std::string("tail"));
    REQUIRE(seg.defined[0].count == 100);
    CHECK_EQ(seg.defined[0].entries[99].value->integer, 99);
    const uint64_t scope = seg.defined[0].scope;

    // Replayed: the same serial, nothing to define.
    CHECK_EQ(pb_vm_run(vm), PB_OK);
    CHECK_EQ(pb_vm_drain_segments(vm, "out", &seg), PB_OK);
    REQUIRE(seg.count == 2);
    CHECK_EQ(seg.defined_count, (size_t)0);
    CHECK_EQ(seg.entries[0].segment, wall);

    // The flat drain still expands it.
    CHECK_EQ(pb_vm_run(vm), PB_OK);
    pb_values flat;
    CHECK_EQ(pb_vm_drain(vm, "out", &flat), PB_OK);
    CHECK_EQ(flat.count, (size_t)101);

    // A host that lost its cache is given it again.
    CHECK_EQ(pb_vm_reset_segments(vm, "out"), PB_OK);
    CHECK_EQ(pb_vm_run(vm), PB_OK);
    CHECK_EQ(pb_vm_drain_segments(vm, "out", &seg), PB_OK);
    CHECK_EQ(seg.defined_count, (size_t)1);

    // A reload records the scope again: a new serial, the same scope id,
    // and the old serial dropped.
    pb_reload_result r;
    CHECK_EQ(pb_vm_reload_source(vm,
                                 "import io\n"
                                 "fn wall(n)\n  for i in range(0, n) do push_output(symbol(\"out\"), i) end\n  nil\nend\n"
                                 "fn other() 1 end\n"
                                 "wall(100)\npush_output(symbol(\"out\"), \"tail\")\n",
                                 &r),
             PB_OK);
    CHECK_EQ(pb_vm_run(vm), PB_OK);
    CHECK_EQ(pb_vm_drain_segments(vm, "out", &seg), PB_OK);
    REQUIRE(seg.defined_count == 1);
    CHECK(seg.defined[0].serial != wall);
    CHECK_EQ(seg.defined[0].scope, scope);
    bool dropped = false;
    for (size_t i = 0; i < seg.dropped_count; ++i) dropped |= seg.dropped[i] == wall;
    CHECK(dropped);
    pb_vm_destroy(vm);
}

TEST(native_takes_named_arguments_once_declared) {
    pb_vm* vm = pb_vm_create();
    auto cb = [](pb_call* call, void*) -> int {
        // a - b, plus c when it was passed: reads by index, as always.
        int64_t v = pb_call_arg(call, 0)->integer - pb_call_arg(call, 1)->integer;
        if (pb_call_arg_count(call) > 2) v += pb_call_arg(call, 2)->integer;
        pb_builder_int(pb_call_result(call), v);
        return 0;
    };
    CHECK_EQ(pb_vm_register_native(vm, "sub", cb, nullptr, nullptr, PB_FX_PURE), PB_OK);
    CHECK_EQ(pb_vm_declare_native_params(vm, "sub", "a, b, c?"), PB_OK);
    CHECK_EQ(pb_vm_declare_native_params(vm, "nope", "a"), PB_ERR_INVALID_ARG);
    CHECK_EQ(pb_vm_declare_native_params(vm, "sub", "a?, b"), PB_ERR_INVALID_ARG);
    CHECK_EQ(pb_vm_load_source(vm,
                               "import io; native fn sub(...args)\npush_output(symbol(\"out\"), sub(b: 1, a: 10))\n"
                               "push_output(symbol(\"out\"), sub(10, c: 5, b: 1))\n",
                               nullptr),
             PB_OK);
    CHECK_EQ(pb_vm_run(vm), PB_OK);
    pb_values out;
    CHECK_EQ(pb_vm_drain(vm, "out", &out), PB_OK);
    REQUIRE(out.count == 2);
    CHECK_EQ(out.items[0].integer, 9);
    CHECK_EQ(out.items[1].integer, 14);
    pb_vm_destroy(vm);
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

// A variant is its tag (`Enum.variant`) and its payload, in both directions:
// one the host builds under a declared enum's tag *is* that variant to the
// script (it matches the pattern, equals the script's own, and reads the
// constant data the declaration gives it), and one the script hands back
// arrives with the same tag and payload.
TEST(enum_variants_cross_the_bridge_by_tag_and_payload) {
    petal::Vm vm;
    vm.load_source(
        "enum Shape\n"
        "  circle(r, sides: 0),\n"
        "  box(w, h, sides: 4),\n"
        "  dot(sides: 0),\n"
        "end\n"
        "fn describe(s: Shape)\n"
        "  match s\n"
        "    when Shape.circle(r) -> \"circle {r} with {s.sides} sides\"\n"
        "    when Shape.box(w, h) -> \"box {w}x{h} with {s.sides} sides, {s.name}\"\n"
        "    when Shape.dot -> \"dot\"\n"
        "  end\n"
        "end\n"
        "fn same(s) s == Shape.box(2, 3) end\n"
        "fn grow(s) Shape.box(s.w * 2, s.h * 2) end\n"
        "fn kind_of(s) type(s) end\n");
    vm.run();

    petal::Builder box;
    box.enum_("Shape.box", [](petal::BuilderRef& e) { e.value(2).value(3); });
    CHECK_EQ(vm.call("describe", box).string(), std::string("box 2x3 with 4 sides, box"));
    CHECK(vm.call("same", box).boolean());
    CHECK_EQ(vm.call("kind_of", box).string(), std::string("Shape"));

    petal::Builder dot;
    dot.enum_("Shape.dot", [](petal::BuilderRef&) {});
    CHECK_EQ(vm.call("describe", dot).string(), std::string("dot"));

    // Back out: the tag is qualified and the payload positional. Constant
    // data belongs to the declaration and is not part of the value.
    petal::Value grown = vm.call("grow", box);
    REQUIRE(grown.is_enum());
    CHECK_EQ(grown.tag(), std::string_view("Shape.box"));
    CHECK_EQ(grown.size(), size_t(2));
    CHECK_EQ(grown[1].integer(), 6);

    // A payload of the wrong size is a variant no arm matches, reported as
    // the script's own error rather than read past its end.
    petal::Builder bad;
    bad.enum_("Shape.box", [](petal::BuilderRef& e) { e.value(2); });
    petal::Error failed = CHECK_THROWS_CODE(vm.call("describe", bad), PB_ERR_RUNTIME);
    CHECK(std::string(failed.what()).find("No matching pattern") != std::string::npos);

    // A tag with no enum in front belongs to no declared enum.
    petal::Builder bare;
    bare.enum_("box", [](petal::BuilderRef& e) { e.value(2).value(3); });
    CHECK(!vm.call("same", bare).boolean());
    CHECK_EQ(vm.call("kind_of", bare).string(), std::string("enum"));
}

// An enum another module declares carries that module in its tag
// (docs/proposals/module-name-collisions.md, part C). The host may still
// build it by the bare tag while the name is unique, and reads the full tag
// back, with pb_enum_tag_display for the printed form.
TEST(module_enum_tags_carry_the_module) {
    petal::Vm vm;
    vm.register_module("cfg",
        "pub enum Kind\n"
        "  park, road(lanes),\n"
        "end\n"
        "pub fn make_road(n) Kind.road(n) end\n");
    vm.register_module("other",
        "pub enum Mode\n"
        "  on,\n"
        "end\n");
    vm.load_source(
        "import cfg\n"
        "import other\n"
        "fn is_park(k) k == cfg.Kind.park end\n"
        "fn shown(k) \"{k}\" end\n"
        "fn road(n) make_road(n) end\n");
    vm.run();

    petal::Builder bare;
    bare.enum_("Kind.park", [](petal::BuilderRef&) {});
    CHECK(vm.call("is_park", bare).boolean());
    petal::Builder full;
    full.enum_("cfg::Kind.park", [](petal::BuilderRef&) {});
    CHECK(vm.call("is_park", full).boolean());
    CHECK_EQ(vm.call("shown", full).string(), std::string("Kind.park"));

    petal::Builder lanes;
    lanes.value(int64_t(2));
    petal::Value road = vm.call("road", lanes);
    REQUIRE(road.is_enum());
    CHECK_EQ(road.tag(), std::string_view("cfg::Kind.road"));
    CHECK_EQ(road.tag_display(), std::string_view("Kind.road"));
    CHECK_EQ(std::string_view(pb_enum_tag_display("cfg::Kind.road")), std::string_view("Kind.road"));
    CHECK_EQ(std::string_view(pb_enum_tag_display("Kind.road")), std::string_view("Kind.road"));
    CHECK(pb_enum_tag_display(nullptr) == nullptr);
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

// A hook that fails must not leave its frames on the stack: each failure's
// trace lists its own calls, however many came before, and so does the first
// failure after a frame that errored.
TEST(a_failed_call_is_not_repeated_in_later_traces) {
    petal::Vm vm;
    vm.load_source(
        "fn find(id)\n  assert(id > 0, \"no such id\")\n  id\nend\n"
        "fn agent_call(id)\n  find(id) + 1\nend\n"
        "fn frame(n)\n  assert(n != 3, \"frame failed\")\nend\n"
        "state n = 0\nn += 1\nframe(n)\n");
    vm.run();
    petal::Builder bad;
    bad.value(0);
    std::string first = CHECK_THROWS_CODE(vm.call("agent_call", bad), PB_ERR_RUNTIME).what();
    CHECK_CONTAINS(first, "in find()");
    for (int i = 0; i < 3; ++i) {
        std::string again = CHECK_THROWS_CODE(vm.call("agent_call", bad), PB_ERR_RUNTIME).what();
        CHECK_EQ(again, first);
    }
    vm.run();
    petal::Error frame = CHECK_THROWS_CODE(vm.run(), PB_ERR_RUNTIME);
    CHECK_CONTAINS(std::string(frame.what()), "in frame()");
    std::string after = CHECK_THROWS_CODE(vm.call("agent_call", bad), PB_ERR_RUNTIME).what();
    CHECK_EQ(after, first);
    petal::Builder good;
    good.value(4);
    CHECK_EQ(vm.call("agent_call", good).integer(), 5);
    vm.run();
}

// Arguments as JSON: by position or by parameter name, with the function's
// declared types deciding int against float and rejecting the wrong kind.
TEST(call_with_json_arguments) {
    petal::Vm vm;
    vm.load_source(
        "enum Kind\n  road(lanes),\n  empty,\nend\n"
        "fn toll(id: string, cents: int = 25, rate: float = 0.5, note = \"-\")\n"
        "  \"{id} {cents} {type(cents)} {rate} {type(rate)} {note}\"\n"
        "end\n"
        "fn lanes(k)\n  match k\n    when Kind.road(n) -> n\n    when _ -> 0\n  end\nend\n"
        "fn total(xs)\n  let t = 0\n  for x in xs do t += x end\n  t\nend\n"
        "fn seven()\n  7\nend\n");
    vm.run();
    CHECK_EQ(vm.call_json("toll", R"({"id": "a"})").string(), std::string("a 25 int 0.5 float -"));
    CHECK_EQ(vm.call_json("toll", R"({"note": "n", "rate": 2, "id": "a"})").string(),
             std::string("a 25 int 2.0 float n"));
    CHECK_EQ(vm.call_json("toll", R"(["b", 40.0, 3])").string(), std::string("b 40 int 3.0 float -"));
    CHECK_EQ(vm.call_json("seven").integer(), 7);
    CHECK_EQ(vm.call_json("total", "[[1, 2, 3]]").integer(), 6);
    CHECK_EQ(vm.call_json("lanes", R"([{"$petal": "enum", "tag": "Kind.road", "data": [3]}])").integer(), 3);

    petal::Error kind = CHECK_THROWS_CODE(vm.call_json("toll", R"({"id": "a", "cents": "abc"})"), PB_ERR_INVALID_ARG);
    CHECK_CONTAINS(std::string(kind.what()), "parameter 'cents' is declared int, got a string");
    CHECK_THROWS_CODE(vm.call_json("toll", R"(["a", 2.5])"), PB_ERR_INVALID_ARG);
    CHECK_THROWS_CODE(vm.call_json("toll", "not json"), PB_ERR_INVALID_ARG);
    CHECK_THROWS_CODE(vm.call_json("nope", "[]"), PB_ERR_NOT_FOUND);
    petal::Error missing = CHECK_THROWS_CODE(vm.call_json("toll", R"({"cents": 1})"), PB_ERR_RUNTIME);
    CHECK_CONTAINS(std::string(missing.what()), "missing a value for parameter 'id'");
    petal::Error unknown = CHECK_THROWS_CODE(vm.call_json("toll", R"({"id": "a", "bogus": 1})"), PB_ERR_RUNTIME);
    CHECK_CONTAINS(std::string(unknown.what()), "no parameter named 'bogus'");

    std::string info = vm.function_json("toll");
    CHECK_CONTAINS(info, R"("name":"cents")");
    CHECK_CONTAINS(info, R"("type":"int")");
    CHECK_CONTAINS(info, R"("default_source":"25")");
    CHECK_THROWS_CODE(vm.function_json("nope"), PB_ERR_NOT_FOUND);
    // The C entry point takes NULL for "no arguments".
    const pb_value* out = nullptr;
    CHECK_EQ(pb_vm_call_json(vm.raw(), "seven", nullptr, &out), PB_OK);
    REQUIRE(out != nullptr);
    CHECK_EQ(out->integer, int64_t(7));
}

// ─── State snapshots ────────────────────────────────────────────────────────

static const char* const kWorld =
    "import io; import vec2\n"
    "enum Kind\n  empty,\n  road(lanes),\n  tower(height, owner),\nend\n"
    "class Tile\n  kind,\n  at,\nend\n"
    "state tick = 0\n"
    "tick += 1\n"
    "state grid = {size: {w: 2, h: 1}, cells: [Tile(Kind.empty, vec2(0, 0)), Tile(Kind.road(2), vec2(1, 0))]}\n"
    "state var log = [\"start\"]\n"
    "state table = freeze({b: [1, 2.5, nil, true], a: {deep: [[1], [2, [3]]]}})\n"
    "state where = vec3(1, 2, 3)\n"
    "state on_done = fn(x) x + 1 end\n"
    "fn bump(key)\n  state count = 0\n  count += tick\n  count\nend\n"
    "let counts = for k in [\"a\", \"b\"] do bump(k) end\n"
    "if tick % 2 == 0 then\n"
    "  grid = {size: grid.size, cells: grid.cells ++ [Tile(Kind.tower(tick, \"me\"), vec2(tick, 0.5))]}\n"
    "  set log = log ++ [\"t{tick}\"]\n"
    "end\n"
    "print(tick, grid, log, table, where, counts, on_done(tick))\n";

static std::string run_printed(petal::Vm& vm) {
    vm.run();
    std::string all;
    for (const std::string& line : vm.take_output()) all += line + "\n";
    return all;
}

TEST(state_round_trips_through_a_document) {
    petal::Vm a;
    a.load_source(kWorld, "world.ptl");
    for (int i = 0; i < 4; ++i) run_printed(a);
    std::string saved = a.save_state();
    CHECK_CONTAINS(saved, R"("format":"petal-state")");
    CHECK_CONTAINS(saved, R"("$petal":"enum")");
    CHECK_CONTAINS(saved, R"("$petal":"instance")");
    // The function is named in the report and absent from the document.
    CHECK_CONTAINS(a.state_report_json(), R"("op":"save")");
    CHECK_CONTAINS(a.state_report_json(), R"({"name":"on_done","holds":"a function"})");
    CHECK(saved.find("on_done") == std::string::npos);
    std::string frame5 = run_printed(a);
    std::string frame6 = run_printed(a);
    CHECK_CONTAINS(frame6, "Kind.tower(6, \"me\")");

    // Another VM running the same program carries on from the save point.
    petal::Vm b;
    b.load_source(kWorld, "world.ptl");
    b.load_state(saved);
    CHECK_CONTAINS(b.state_report_json(), R"("dropped":[])");
    CHECK_EQ(run_printed(b), frame5);
    CHECK_EQ(run_printed(b), frame6);
    CHECK_EQ(b.save_state(), a.save_state());

    // Undo: the VM that saved goes back, as often as it likes.
    for (int i = 0; i < 2; ++i) {
        a.load_state(saved);
        CHECK_EQ(run_printed(a), frame5);
        CHECK_EQ(run_printed(a), frame6);
    }
    CHECK_EQ(a.state("tick").integer(), 6);

    // Through a file.
    fs::path file = scratch_dir("state_round_trips") / "save.json";
    a.load_state(saved);
    a.save_state_file(file.string());
    run_printed(a);
    a.load_state_file(file.string());
    CHECK_EQ(run_printed(a), frame5);
    CHECK_THROWS_CODE(a.load_state_file((file.parent_path() / "nope.json").string()), PB_ERR_IO);
}

TEST(state_loads_across_a_code_edit) {
    const char* v1 = "state kept = 0\nkept += 1\nstate removed = 100\nremoved += 1\nprint(kept, removed)\n";
    const char* v2 = "state kept = 0\nkept += 1\nstate added = 50\nadded += 1\nprint(kept, added)\n";
    petal::Vm vm;
    vm.load_source(v1, "main.ptl");
    run_printed(vm);
    CHECK_EQ(run_printed(vm), std::string("2 102\n"));
    std::string saved = vm.save_state();

    petal::Vm edited;
    edited.load_source(v2, "main.ptl");
    edited.load_state(saved);
    CHECK_CONTAINS(edited.state_report_json(), R"("dropped":["removed"])");
    CHECK_CONTAINS(edited.state_report_json(), R"("slots":1)");
    CHECK_EQ(run_printed(edited), std::string("3 51\n"));
    CHECK(edited.save_state().find("removed") == std::string::npos);
}

TEST(a_bad_state_document_is_rejected_and_changes_nothing) {
    petal::Vm vm;
    CHECK_THROWS_CODE(vm.save_state(), PB_ERR_NOT_LOADED);
    CHECK_THROWS_CODE(vm.load_state("{}"), PB_ERR_NOT_LOADED);
    CHECK_EQ(vm.state_report_json(), std::string(""));
    vm.load_source("state n = 0\nn += 1\nprint(n)\n");
    CHECK_EQ(run_printed(vm), std::string("1\n"));
    CHECK_THROWS_CODE(vm.load_state("{"), PB_ERR_INVALID_ARG);
    CHECK_THROWS_CODE(vm.load_state("[1]"), PB_ERR_INVALID_ARG);
    petal::Error other = CHECK_THROWS_CODE(
        vm.load_state(R"({"format": "petal-state", "version": 1, "slots": [{"name": "x", "value": 1}]})"),
        PB_ERR_INVALID_ARG);
    CHECK_CONTAINS(std::string(other.what()), "written by a different script");
    CHECK_EQ(run_printed(vm), std::string("2\n"));
    // A hand-written document, by name.
    vm.load_state(R"({"format": "petal-state", "version": 1, "slots": [{"name": "n", "value": 40}]})");
    CHECK_EQ(run_printed(vm), std::string("41\n"));
}

TEST(diagnostics_as_json) {
    petal::Vm vm;
    CHECK_THROWS_CODE(vm.diagnostics_json(), PB_ERR_NOT_LOADED);
    vm.load_source("let n: int = \"one\"\nprint(n)\n", "main.ptl");
    std::string json = vm.diagnostics_json();
    CHECK_CONTAINS(json, R"("warnings":1)");
    CHECK_CONTAINS(json, R"("severity":"warning")");
    CHECK_CONTAINS(json, R"("line":1)");
    vm.reload_source("let n: int = 1\nprint(n)\n");
    CHECK_CONTAINS(vm.diagnostics_json(), R"("items":[])");
}

// After a reload a host prints what the edit introduced, and a standing
// warning is not said again.
TEST(reload_reports_the_findings_the_edit_introduced) {
    std::string base = "fn double(x: int) x end\nlet a = double(\"one\")\nlet b = 4\nprint(a, b)\n";
    petal::Vm vm;
    vm.load_source(base, "main.ptl");
    CHECK_CONTAINS(vm.diagnostics_json(), R"("warnings":1)");
    CHECK_EQ(vm.new_diagnostics_text(), std::string(""));

    std::string edited = base;
    edited.replace(edited.find("let b = 4"), 9, "// moved\nlet b: string = 4");
    vm.reload_source(edited);
    CHECK_CONTAINS(vm.diagnostics_json(), R"("warnings":2)");
    std::string introduced = vm.new_diagnostics_text();
    CHECK_CONTAINS(introduced, "warning:");
    CHECK_CONTAINS(introduced, "line 4");
    CHECK(introduced.find("line 2") == std::string::npos);
    CHECK_EQ(vm.new_diagnostics_text(true), std::string(""));
    CHECK_CONTAINS(vm.last_reload_json(), R"("new_diagnostics":[{"severity":"warning",)");

    edited.replace(edited.find("print(a, b)"), 11, "print(b, a)");
    vm.reload_source(edited);
    CHECK_EQ(vm.new_diagnostics_text(), std::string(""));
    CHECK_CONTAINS(vm.last_reload_json(), R"("new_diagnostics":[])");
}

// ─── petal-ui ───────────────────────────────────────────────────────────────

TEST(ui_draw_follows_mouse) {
    petal::Vm vm;
    vm.set_dimensions(800, 600);
    vm.load_source(
        "import ui\nclear(10, 20, 30)\n"
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
    // The `ui` package is registered: `import ui` brings in button().
    petal::Vm vm;
    vm.set_dimensions(400, 300);
    vm.load_source(
        "import ui\n"
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

// A segmented draw buffer drained with pb_vm_drain_draw: the bridge keeps
// each segment's commands decoded and copies a replayed panel's from that
// cache. The commands are those of a VM that runs without memo.
TEST(ui_draw_through_segments_matches_the_unmemoized_frames) {
    const char* src =
        "import ui\n"
        "import vec2\n"
        "state n = 0\n"
        "n += 1\n"
        "fn rows(k)\n"
        "  for i in range(0, 30) do draw_rect({x: 4, y: i * 10, w: 50 + k, h: 8}, #336699) end\n"
        "  draw_text(\"rows {k}\", vec2(4, 2), 12, #ffffff)\n"
        "  nil\n"
        "end\n"
        "rows(1)\n"
        "rows(n % 2)\n"
        "draw_circle({x: 9, y: 9}, float(n), #ff0000)\n";
    const auto flat_of = [](petal::Vm& vm) {
        std::vector<std::tuple<int, int, int, int, int, std::string>> out;
        for (const pb_draw_cmd& d : vm.drain_draw())
            out.emplace_back(int(d.kind), d.x, d.y, d.w, d.radius, d.text ? std::string(d.text) : std::string());
        return out;
    };
    petal::Vm plain, seg;
    for (petal::Vm* vm : {&plain, &seg}) {
        vm->set_dimensions(400, 400);
        vm->load_source(src);
    }
    plain.set_memo(false);
    seg.set_segmented("draw_commands", true);
    for (int f = 0; f < 5; ++f) {
        if (f == 3) seg.reset_segments("draw_commands");   // a host that lost its cache
        plain.begin_frame(0.016, f, f * 0.016);
        seg.begin_frame(0.016, f, f * 0.016);
        plain.run();
        seg.run();
        const auto want = flat_of(plain);
        CHECK(want.size() > 60);
        CHECK(flat_of(seg) == want);
    }
}

TEST(ui_keyboard_and_relative_mouse) {
    petal::Vm vm;
    vm.load_source(
        "import io; import ui\nlet out = symbol(\"out\")\n"
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
        "import io; import ui\n"
        "native fn draw_sprite(...args)\n"
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
    vm.load_source("import io; import ui\npush_output(symbol(\"out\"), text_width(\"abc\", 10))\n");
    CHECK_EQ(run_out(vm), std::vector<double>{2.0 + 10.0 + 5.0});
    // An empty table is uniform again.
    vm.set_text_advances({});
    CHECK_EQ(run_out(vm), std::vector<double>{15.0});
}

TEST(seeded_random_is_reproducible) {
    auto sample = [] {
        petal::Vm vm;
        vm.set_seed(1234);
        vm.load_source("import io; push_output(symbol(\"out\"), random(0, 1000000))\n");
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
    vm.load_source("import io; push_output(symbol(\"out\"), 1)\n");
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
        "import io; state frames = 0\n"
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

// ─── Modules, packages, host interfaces ─────────────────────────────────────

TEST(registered_modules_are_reached_by_import) {
    petal::Vm vm;
    vm.register_module("engine", "pub fn twice(x)\n  x * 2\nend\n");
    vm.register_module("util", "pub fn inc(x)\n  x + 1\nend\n");
    vm.load_source(
        "import io; import ui\n"
        "import engine\n"
        "import util\n"
        "push_output(symbol(\"out\"), twice(util.inc(20)))\n"
        "draw_rect(1, 2, 3, 4, 5, 6, 7)\n");
    CHECK_EQ(run_out(vm), std::vector<double>{42});

    // Nothing is imported implicitly: without the import the name is unknown.
    vm.load_source("import io; push_output(symbol(\"out\"), twice(1))\n");
    CHECK_THROWS_CODE(vm.run(), PB_ERR_RUNTIME);
}

TEST(a_package_registers_from_memory) {
    petal::Vm vm;
    vm.register_package("kit", {
        {"kit", "pub import ./shapes\npub fn two()\n  2\nend\n"},
        {"shapes", "pub fn area(w, h)\n  w * h\nend\n"},
    });
    vm.load_source(
        "import io; import kit\n"
        "import kit/shapes as s\n"
        "push_output(symbol(\"out\"), area(two(), 3) + s.area(1, 1))\n");
    CHECK_EQ(run_out(vm), std::vector<double>{7});
    // A name that is not an identifier is refused.
    CHECK_THROWS_CODE(vm.register_package("not a name", {}), PB_ERR_INVALID_ARG);
}

TEST(a_host_interface_package_declares_its_natives) {
    petal::Vm vm;
    vm.native("gravity_at", [](petal::Call& call) { call.result().value(-10.0 * call[0].number()); });
    // Registered but not declared: no script can reach it, and the check says so.
    CHECK(vm.verify_interface({"ui"}).find("gravity_at") != std::string::npos);
    vm.load_source("import io; push_output(symbol(\"out\"), gravity_at(2))\n");
    CHECK_THROWS_CODE(vm.run(), PB_ERR_RUNTIME);

    // The interface package declares it; scripts import the package.
    vm.register_package("game", {{"game", "pub native fn gravity_at(frame: num) -> float\n"}});
    CHECK_EQ(vm.verify_interface({"ui", "game"}), std::string());
    vm.load_source("import io; import game\npush_output(symbol(\"out\"), gravity_at(frame: 2))\n");
    CHECK_EQ(run_out(vm), std::vector<double>{-20});

    // A declared native the host does not register is reported, and a script
    // that uses it does not load — unless the VM is only checking.
    vm.register_package("extra", {{"extra", "pub native fn missing_one()\n"}});
    CHECK(vm.verify_interface({"ui", "game", "extra"}).find("missing_one") != std::string::npos);
    CHECK_THROWS_CODE(vm.load_source("import extra\nmissing_one()\n"), PB_ERR_COMPILE);
    vm.set_check_only(true);
    vm.load_source("import extra\nmissing_one()\n");
    vm.set_check_only(false);
}

TEST(a_package_requires_an_interface_the_vm_provides) {
    petal::Vm vm;
    // The manifest rides along with the sources and says what the package needs.
    vm.register_package("kit", {{"kit", "pub fn two()\n  2\nend\n"},
                                {"petal.toml", "[package]\nname = \"kit\"\nversion = \"1.0.0\"\n"
                                               "[requires]\ngame = \">=0.2\"\n"}});
    const char* script = "import io; import kit\npush_output(symbol(\"out\"), two())\n";
    // Not provided: a load error before any line runs.
    CHECK_THROWS_CODE(vm.load_source(script), PB_ERR_COMPILE);
    // Provided too old: still refused.
    vm.provides("game", "0.1");
    CHECK_THROWS_CODE(vm.load_source(script), PB_ERR_COMPILE);
    vm.provides("game", "0.2.0");
    vm.load_source(script);
    CHECK_EQ(run_out(vm), std::vector<double>{2});
    CHECK_THROWS_CODE(vm.provides("game", "soon"), PB_ERR_INVALID_ARG);

    // An interface package provides itself at its manifest's version.
    petal::Vm other;
    other.register_package("game", {{"game", ""},
                                    {"petal.toml", "[package]\nname = \"game\"\nversion = \"0.3.0\"\n"}});
    other.provides_package("game");
    other.register_package("kit", {{"kit", "pub fn two()\n  2\nend\n"},
                                   {"petal.toml", "[package]\nname = \"kit\"\n[requires]\ngame = \"^0.3\"\n"
                                                  "ui = \">=0.1\"\n"}});
    // A pb_vm_create VM provides `ui` already.
    other.load_source(script);
    CHECK_EQ(run_out(other), std::vector<double>{2});
    CHECK_THROWS_CODE(other.provides_package("nope"), PB_ERR_NOT_FOUND);
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
               "import io; state count = 0\n"
               "count += 1\n"
               "push_output(symbol(\"out\"), count * 1)\n");
    petal::Vm vm;
    vm.load_file(main.string());
    CHECK_EQ(run_out(vm), std::vector<double>{1});
    CHECK_EQ(run_out(vm), std::vector<double>{2});
    CHECK(!vm.sources_changed());

    // Edit a constant while "running".
    write_file(main,
               "import io; state count = 0\n"
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
    write_file(main, "import io; state count = 0\ncount += 1\npush_output(symbol(\"out\"), count)\n");
    r = vm.reload();
    CHECK(r.state_preserved >= 1u);
    CHECK_EQ(r.state_dropped, 1u);  // `extra`
    CHECK_EQ(run_out(vm), std::vector<double>{5});
}

// A file override stands in for a file on disk wherever an import resolves to
// it: a relative import is a file, so a registered module cannot replace it.
TEST(a_file_override_replaces_an_imported_file) {
    fs::path dir = scratch_dir("file_override");
    write_file(dir / "tuning.ptl", "pub let SPEED = 2\n");
    write_file(dir / "main.ptl",
               "import io; import ./tuning\n"
               "push_output(symbol(\"out\"), tuning.SPEED)\n");
    {
        petal::Vm vm;
        vm.load_file((dir / "main.ptl").string());
        CHECK_EQ(run_out(vm), std::vector<double>{2});
    }
    petal::Vm vm;
    // A module registered in memory under the same name is not what `./tuning` means...
    vm.register_module("tuning", "pub let SPEED = 5\n");
    vm.load_file((dir / "main.ptl").string());
    CHECK_EQ(run_out(vm), std::vector<double>{2});
    // ...the override is, from the next load on.
    vm.override_file((dir / "tuning.ptl").string(), "pub let SPEED = 7\n");
    vm.load_file((dir / "main.ptl").string());
    CHECK_EQ(run_out(vm), std::vector<double>{7});
    // The file on disk is untouched, and its text is not what runs while the
    // override stands: a reload after an edit still compiles the override.
    write_file(dir / "tuning.ptl", "pub let SPEED = 100\n");
    vm.reload();
    CHECK_EQ(run_out(vm), std::vector<double>{7});
    // A later override replaces the earlier one, at the next load or reload.
    vm.override_file((dir / "tuning.ptl").string(), "pub let SPEED = 9\n");
    vm.reload();
    CHECK_EQ(run_out(vm), std::vector<double>{9});
}

TEST(hot_reload_tracks_imported_modules) {
    fs::path dir = scratch_dir("hot_reload_imports");
    write_file(dir / "tuning.ptl", "pub let SPEED = 2\n");
    write_file(dir / "main.ptl",
               "import io; import ./tuning\n"
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
    write_file(dir / "tuning.ptl", "pub let SPEED = 10\n");
    CHECK(vm.sources_changed());
    // Only the edited module is named, with the path source_files() gives it.
    CHECK_EQ(vm.changed_sources(), std::vector<std::string>{files[1]});
    vm.reload();
    CHECK(vm.changed_sources().empty());
    CHECK_EQ(run_out(vm), std::vector<double>{12});

    // Both files edited: named in source_files() order.
    write_file(dir / "tuning.ptl", "pub let SPEED = 1\n");
    write_file(dir / "main.ptl",
               "import io; import ./tuning\n"
               "state pos = 0\n"
               "pos += tuning.SPEED * 2\n"
               "push_output(symbol(\"out\"), pos)\n");
    CHECK_EQ(vm.changed_sources(), files);
}

TEST(hot_reload_watches_new_imports_and_deletions) {
    fs::path dir = scratch_dir("hot_reload_watch");
    write_file(dir / "main.ptl", "import io; push_output(symbol(\"out\"), 1)\n");
    write_file(dir / "extra.ptl", "pub let BONUS = 5\n");
    petal::Vm vm;
    vm.load_file((dir / "main.ptl").string());
    CHECK_EQ(vm.source_files().size(), size_t(1));

    // The reloaded program imports a new module: it is watched from then on.
    write_file(dir / "main.ptl", "import io; import ./extra\npush_output(symbol(\"out\"), extra.BONUS)\n");
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
    write_file(dir / "extra.ptl", "pub let BONUS = 7\n");
    CHECK(vm.sources_changed());
    vm.reload();
    CHECK_EQ(run_out(vm), std::vector<double>{7});
}

// A reload does only the work the edit calls for, and a value can be set in
// the running program without the file changing.
TEST(hot_reload_patches_values_and_set_config_writes_no_file) {
    fs::path dir = scratch_dir("hot_reload_config");
    fs::path main = dir / "game.ptl";
    fs::path config = dir / "config.ptl";
    write_file(config, "// knobs\npub config let POST = {bloom: 0.25, steps: [1, 2]}\n");
    write_file(main,
               "import io; import ./config\n"
               "state frames = 0\n"
               "frames += 1\n"
               "push_output(symbol(\"out\"), config.POST.bloom * 100 + config.POST.steps[1])\n");
    petal::Vm vm;
    vm.load_file(main.string());
    CHECK_EQ(run_out(vm), std::vector<double>{27});

    // A value edit: written in place.
    write_file(config, "// knobs\npub config let POST = {bloom: 0.5, steps: [1, 2]}\n");
    CHECK(vm.sources_changed());
    petal::ReloadResult r = vm.reload();
    CHECK_EQ(r.kind, PB_RELOAD_PATCHED);
    CHECK_EQ(r.state_dropped, 0u);
    CHECK(vm.last_reload_json().find("\"path\":\"POST.bloom\"") != std::string::npos);
    // What became of the static values: the edited table is rebuilt.
    CHECK(vm.last_reload_json().find("\"statics\":{") != std::string::npos);
    CHECK(vm.last_reload_json().find("\"stale_seeds\":[]") != std::string::npos);
    CHECK_EQ(run_out(vm), std::vector<double>{52});

    // A layout edit: nothing but positions.
    write_file(config, "// knobs\n\npub config let POST = {\n  bloom: 0.5,\n  steps: [1, 2],\n}\n");
    CHECK_EQ(vm.reload().kind, PB_RELOAD_RELOCATED);
    CHECK_EQ(run_out(vm), std::vector<double>{52});

    // A drag: no file write.
    // (A bare JSON number is a float; an integer is {"int": n}.)
    CHECK(vm.set_tunable("POST.steps[1]", "{\"int\":9}"));
    CHECK(vm.set_tunable("POST.bloom", "0.75", config.string()));
    CHECK(!vm.sources_changed());
    CHECK_EQ(vm.last_reload_kind(), PB_RELOAD_PATCHED);
    CHECK_EQ(run_out(vm), std::vector<double>{84});
    // A value of another type needs the file and a reload.
    CHECK(!vm.set_tunable("POST.steps[1]", "9.5"));
    CHECK_THROWS_CODE(vm.set_tunable("POST.missing.deep", "1"), PB_ERR_NOT_FOUND);
    CHECK_EQ(run_out(vm), std::vector<double>{84});

    // The drag ends: write the running text; the reload finds nothing to do.
    std::string text = vm.source_text(config.string());
    CHECK(text.find("bloom: 0.75,") != std::string::npos);
    CHECK(text.find("steps: [1, 9],") != std::string::npos);
    write_file(config, text);
    CHECK_EQ(vm.reload().kind, PB_RELOAD_UNCHANGED);
    CHECK_EQ(run_out(vm), std::vector<double>{84});

    // Anything structural is still a recompile, and state survives it.
    write_file(config, "pub config let POST = {bloom: 0.75, steps: [1, 9, 3]}\n");
    r = vm.reload();
    CHECK_EQ(r.kind, PB_RELOAD_RECOMPILED);
    CHECK_EQ(r.state_dropped, 0u);
    CHECK_EQ(run_out(vm), std::vector<double>{84});
}

// A first load that fails leaves nothing loaded, but the VM watches for the
// fix: the entry file and any .ptl file beside it, and reload() retries the
// load.
TEST(failed_load_file_is_watched_and_retried) {
    fs::path dir = scratch_dir("failed_load");
    fs::path main = dir / "main.ptl";
    write_file(main, "import io; import ./helper\npush_output(symbol(\"out\"), helper.N)\n");
    petal::Vm vm;
    CHECK_THROWS_CODE(vm.load_file(main.string()), PB_ERR_COMPILE);  // no helper.ptl yet
    CHECK(!vm.loaded());
    CHECK(!vm.sources_changed());

    // Creating the missing module is the fix.
    write_file(dir / "helper.ptl", "pub let N = 4\n");
    CHECK(vm.sources_changed());
    auto changed = vm.changed_sources();
    REQUIRE(changed.size() == 1);
    CHECK_EQ(fs::path(changed[0]).filename().string(), std::string("helper.ptl"));
    petal::ReloadResult r = vm.reload();
    CHECK_EQ(r.state_preserved, 0u);
    CHECK(vm.loaded());
    CHECK(!vm.sources_changed());
    CHECK_EQ(run_out(vm), std::vector<double>{4});

    // A missing entry file is watched too: it appearing is the change.
    fs::path later = dir / "later.ptl";
    petal::Vm vm2;
    CHECK_THROWS_CODE(vm2.load_file(later.string()), PB_ERR_IO);
    CHECK(!vm2.sources_changed());
    write_file(later, "import io; push_output(symbol(\"out\"), 9)\n");
    CHECK(vm2.sources_changed());
    vm2.reload();
    CHECK_EQ(run_out(vm2), std::vector<double>{9});
}

// After a failed reload the old program keeps running and its files stay
// watched, and so does every .ptl file under the entry's directory: the fix
// may land in a module the broken edit newly imports.
TEST(failed_reload_watches_the_script_directory) {
    fs::path dir = scratch_dir("failed_reload_dir");
    fs::path main = dir / "main.ptl";
    fs::create_directories(dir / "lib");
    write_file(dir / "lib" / "shapes.ptl", "pub let SIDES = (\n");  // broken, not imported yet
    write_file(main, "import io; state n = 0\nn += 1\npush_output(symbol(\"out\"), n)\n");
    petal::Vm vm;
    vm.load_file(main.string());
    CHECK_EQ(run_out(vm), std::vector<double>{1});
    // An edit in a file the program does not import is not a change yet.
    write_file(dir / "lib" / "shapes.ptl", "pub let SIDES = (\n\n");
    CHECK(!vm.sources_changed());

    // The edit imports the broken module: the reload fails.
    write_file(main, "import io; import ./lib/shapes\nstate n = 0\nn += shapes.SIDES\npush_output(symbol(\"out\"), n)\n");
    CHECK(vm.sources_changed());
    CHECK_THROWS_CODE(vm.reload(), PB_ERR_COMPILE);
    CHECK(!vm.sources_changed());  // this version was looked at
    CHECK_EQ(run_out(vm), std::vector<double>{2});

    // Fixing the module (which the old program never imported) is seen.
    write_file(dir / "lib" / "shapes.ptl", "pub let SIDES = 6\n");
    CHECK(vm.sources_changed());
    auto changed = vm.changed_sources();
    REQUIRE(changed.size() == 1);
    CHECK_EQ(fs::path(changed[0]).filename().string(), std::string("shapes.ptl"));
    petal::ReloadResult r = vm.reload();
    CHECK(r.state_preserved >= 1u);  // `n` kept
    CHECK_EQ(run_out(vm), std::vector<double>{8});

    // Healthy again: only the program's own files are watched.
    write_file(dir / "unrelated.ptl", "pub let X = 1\n");
    CHECK(!vm.sources_changed());
}

TEST(reload_from_source) {
    petal::Vm vm;
    vm.native("host_scale", [](petal::Call& c) { c.result().value(c[0].number() * 10); });
    vm.load_source("import io; state n = 1\nn += 1\npush_output(symbol(\"out\"), n)\n");
    CHECK_EQ(run_out(vm), std::vector<double>{2});
    petal::ReloadResult r = vm.reload_source("import io; native fn host_scale(n)\nstate n = 1\nn += 1\npush_output(symbol(\"out\"), host_scale(n))\n");
    CHECK(r.state_preserved >= 1u);
    CHECK_EQ(r.state_dropped, 0u);
    CHECK_EQ(run_out(vm), std::vector<double>{30});
    // pb_vm_reload needs a file-backed program.
    CHECK_THROWS_CODE(vm.reload(), PB_ERR_INVALID_ARG);
}

// ─── Input scenarios ────────────────────────────────────────────────────────

TEST(source_reads_bindings_and_edits_by_path) {
    const std::string text =
        "// The look.\n"
        "config let POST = {\n"
        "  exposure: 1.45,   // brighter than day\n"
        "  effects: [{effect: \"grain\", amount: 0.3}],\n"
        "}\n"
        "let TINT = #ff2e88\n";
    petal::Source src(text);
    const std::optional<std::string> json = src.bindings_json();
    REQUIRE(json.has_value());
    CHECK_CONTAINS(*json, "\"name\":\"POST\",\"config\":true");
    CHECK_CONTAINS(*json, "\"comment\":\"The look.\"");
    CHECK_CONTAINS(*json, "\"exposure\":1.45");
    CHECK_EQ(src.expr("TINT").value_or(""), std::string("#ff2e88"));
    CHECK_EQ(src.expr("POST.effects[0].amount").value_or(""), std::string("0.3"));

    CHECK(src.set("POST.exposure", "1.2"));
    CHECK(src.set("POST.effects[0].amount", "0.55"));
    CHECK(src.set("TINT", "#29d9ff"));
    CHECK_EQ(src.text(), std::string("// The look.\n"
                                     "config let POST = {\n"
                                     "  exposure: 1.2,   // brighter than day\n"
                                     "  effects: [{effect: \"grain\", amount: 0.55}],\n"
                                     "}\n"
                                     "let TINT = #29d9ff\n"));

    // A refused edit reports why and changes nothing.
    const std::string before = src.text();
    CHECK(!src.set("POST.bloom", "1.0"));
    CHECK_CONTAINS(src.error(), "POST.bloom");
    CHECK(!src.set("POST.exposure", "1 +"));
    CHECK_EQ(src.text(), before);
    CHECK(!src.expr("NOPE").has_value());

    // Text that does not parse is reported, not thrown.
    petal::Source broken("let x = (\n");
    CHECK(!broken.bindings_json().has_value());
    CHECK(!broken.error().empty());
}

// `config let X = tunable(v, min, max, step)`: hosts see `v` and the range,
// and the path `X` names `v`, live and in the text.
TEST(a_tunable_config_reports_its_range_and_sets_by_name) {
    const std::string text =
        "config let SPEED = tunable(4.0, 0, 10)   // px per frame\n"
        "config let GRAV = tunable(-9.8, min: -20, max: 0, step: 0.1)\n"
        "config let PLAIN = 3\n";
    petal::Source src(text);
    const std::optional<std::string> json = src.bindings_json();
    REQUIRE(json.has_value());
    CHECK_CONTAINS(*json, "\"name\":\"SPEED\",\"config\":true");
    CHECK_CONTAINS(*json, "\"text\":\"4.0\",\"value\":4.0,\"tunable\":{\"min\":0.0,\"max\":10.0},"
                          "\"range\":{\"min\":0.0,\"max\":10.0}");
    CHECK_CONTAINS(*json, "\"value\":-9.8,\"tunable\":{\"min\":-20.0,\"max\":0.0,\"step\":0.1},"
                          "\"range\":{\"min\":-20.0,\"max\":0.0,\"step\":0.1}");
    CHECK_CONTAINS(*json, "\"value\":{\"int\":3},\"tunable\":{}}");
    CHECK_EQ(src.expr("SPEED").value_or(""), std::string("4.0"));
    CHECK(src.set_value("SPEED", "5.5"));
    CHECK(src.set_value("GRAV", "1.5"));
    CHECK_EQ(src.text(), std::string("config let SPEED = tunable(5.5, 0, 10)   // px per frame\n"
                                     "config let GRAV = tunable(1.5, min: -20, max: 0, step: 0.1)\n"
                                     "config let PLAIN = 3\n"));

    // Live: the wrapped value takes a drag, across zero too, with no reload,
    // and it is not clamped to the range.
    petal::Vm vm;
    vm.load_source("import io\nconfig let SPEED = tunable(4.0, 0, 10)\n"
                   "push_output(symbol(\"out\"), SPEED)\n");
    CHECK_EQ(run_out(vm), std::vector<double>{4});
    CHECK(vm.set_tunable("SPEED", "-2.5"));
    CHECK_EQ(vm.last_reload_kind(), PB_RELOAD_PATCHED);
    CHECK_EQ(run_out(vm), std::vector<double>{-2.5});
    CHECK(vm.set_tunable("SPEED", "12.0"));
    CHECK_EQ(run_out(vm), std::vector<double>{12});
}

// Settings written without `config`: `let X = tunable(v, ...)`, of any data.
// The "tunable" section says what was written; "config" and "range" mirror it.
TEST(tunable_settings_have_a_tunable_section_and_set_by_name) {
    const std::string text =
        "let START = tunable(\"title\", options: [\"title\", \"play\"])  // first screen\n"
        "let SPEED = tunable(4.0, 0, 10)\n"
        "let DEBUG = tunable(false)\n"
        "pub let LOOK = tunable({bloom: 0.3, steps: [1, 2]})\n"
        "let PLAIN = 2\n";
    petal::Source src(text);
    const std::optional<std::string> json = src.bindings_json();
    REQUIRE(json.has_value());
    CHECK_CONTAINS(*json, "\"name\":\"START\",\"config\":true");
    CHECK_CONTAINS(*json, "\"value\":\"title\",\"tunable\":{\"options\":[\"title\",\"play\"]}}");
    CHECK_CONTAINS(*json, "\"value\":4.0,\"tunable\":{\"min\":0.0,\"max\":10.0},\"range\":{\"min\":0.0,\"max\":10.0}");
    CHECK_CONTAINS(*json, "\"value\":false,\"tunable\":{}}");
    CHECK_CONTAINS(*json, "\"name\":\"LOOK\",\"config\":true,\"exported\":true");
    CHECK_CONTAINS(*json, "\"name\":\"PLAIN\",\"config\":false");
    CHECK(json->find("\"value\":{\"int\":2},\"tunable\"") == std::string::npos);

    // Set by name: only the value changes.
    CHECK(src.set_setting("START", "\"play\""));
    CHECK(src.set_setting("DEBUG", "true"));
    CHECK(src.set_setting("LOOK", "{\"rec\":{\"bloom\":0.5,\"steps\":[{\"int\":1},{\"int\":2}]}}"));
    CHECK(!src.set_setting("PLAIN", "{\"int\":3}"));
    CHECK(!src.error().empty());
    CHECK(!src.set_setting("NOPE", "1.0"));
    CHECK_EQ(src.text(), std::string("let START = tunable(\"play\", options: [\"title\", \"play\"])  // first screen\n"
                                     "let SPEED = tunable(4.0, 0, 10)\n"
                                     "let DEBUG = tunable(true)\n"
                                     "pub let LOOK = tunable({bloom: 0.5, steps: [1, 2]})\n"
                                     "let PLAIN = 2\n"));

    // Live: set_tunable, and the old C name still answers.
    petal::Vm vm;
    vm.load_source("import io\nlet SPEED = tunable(4.0, 0, 10)\nlet LOOK = tunable({bloom: 0.25})\n"
                   "push_output(symbol(\"out\"), SPEED + LOOK.bloom)\n");
    CHECK_EQ(run_out(vm), std::vector<double>{4.25});
    CHECK(vm.set_tunable("SPEED", "6.0"));
    CHECK_EQ(vm.last_reload_kind(), PB_RELOAD_PATCHED);
    CHECK_CONTAINS(vm.last_reload_json(), "\"tunable\":true");
    CHECK_EQ(pb_vm_set_config(vm.raw(), nullptr, "LOOK.bloom", "0.5"), PB_OK);
    CHECK_EQ(run_out(vm), std::vector<double>{6.5});
    CHECK_CONTAINS(vm.source_text(), "let SPEED = tunable(6.0, 0, 10)\nlet LOOK = tunable({bloom: 0.5})\n");
}

TEST(source_edits_values_in_place_keeping_style) {
    const std::string text =
        "// The look.\n"
        "config let POST = {\n"
        "  exposure: 1.450,   // brighter than day\n"
        "  tint: #FF2E88,\n"
        "  effects: [\n"
        "    {effect: \"grain\", amount: 0.3},\n"
        "    // curved glass\n"
        "    {effect: \"crt\"},\n"
        "  ],\n"
        "}\n"
        "let DROPS = 450\n";
    petal::Source src(text);
    const std::optional<std::string> json = src.bindings_json();
    REQUIRE(json.has_value());
    // A color literal reads as a color.
    CHECK_CONTAINS(*json, "\"tint\":{\"color\":\"#ff2e88\"}");

    // Writing back what is there moves nothing.
    CHECK(src.set_value("POST.exposure", "1.45"));
    CHECK(src.set_value("POST.tint", "{\"color\":\"#ff2e88\"}"));
    CHECK(src.set_value("POST.effects", "[{\"rec\":{\"effect\":\"grain\",\"amount\":0.3}},{\"rec\":{\"effect\":\"crt\"}}]"));
    CHECK(src.remove("POST.absent"));
    CHECK_EQ(src.text(), text);

    // Scalars keep the old token's habits; new elements copy their siblings.
    CHECK(src.set_value("POST.exposure", "1.2"));
    CHECK(src.set_value("POST.tint", "{\"color\":\"#29d9ff\"}"));
    CHECK(src.set_value("DROPS", "{\"int\":500}"));
    CHECK(src.append("POST.effects", "{\"rec\":{\"effect\":\"halftone\",\"ink\":{\"color\":\"#101018\"}}}"));
    CHECK(src.remove("POST.effects[0]"));
    CHECK(src.insert("POST.bloom", "0.4", "tint"));
    CHECK_EQ(src.text(), std::string("// The look.\n"
                                     "config let POST = {\n"
                                     "  exposure: 1.200,   // brighter than day\n"
                                     "  bloom: 0.4,\n"
                                     "  tint: #29D9FF,\n"
                                     "  effects: [\n"
                                     "    // curved glass\n"
                                     "    {effect: \"crt\"},\n"
                                     "    {effect: \"halftone\", ink: #101018},\n"
                                     "  ],\n"
                                     "}\n"
                                     "let DROPS = 500\n"));

    // The inverse edits restore the original bytes.
    CHECK(src.remove("POST.bloom"));
    CHECK(src.remove("POST.effects[1]"));
    CHECK(src.insert("POST.effects[0]", "{\"rec\":{\"effect\":\"grain\",\"amount\":0.3}}"));
    CHECK(src.set_value("POST.exposure", "1.45"));
    CHECK(src.set_value("POST.tint", "{\"color\":\"#ff2e88\"}"));
    CHECK(src.set_value("DROPS", "{\"int\":450}"));
    CHECK_EQ(src.text(), text);

    // A refused edit reports why and changes nothing.
    CHECK(!src.set_value("POST.effects[7].amount", "1.0"));
    CHECK_CONTAINS(src.error(), "POST.effects[7].amount");
    CHECK(!src.set_value("POST.exposure", "{\"oops\":1}"));
    CHECK(!src.insert("POST.effects[9]", "1.0"));
    CHECK(!src.append("DROPS", "1.0"));
    CHECK(!src.remove("POST.effects[2]"));
    CHECK_EQ(src.text(), text);
    CHECK_EQ(pb_source_set_value(src.raw(), nullptr, "1"), PB_ERR_INVALID_ARG);
    CHECK_EQ(pb_source_remove(nullptr, "x"), PB_ERR_INVALID_ARG);
}

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
    vm.load_source(R"petal(import ui
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
    vm.load_source(R"petal(import ui
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

// ─── petal-ui: batches, retained layers, targets ────────────────────────────

static const char* RETAINED_UI = R"petal(import ui
cached_layer(1, rect(10, 20, 30, 40), fn()
  draw_rects([0, 0, 4, 4, 0, 4, 0, 4, 4, 1], [#ff0000, {r: 0, g: 255, b: 0, a: 128}])
end)
state n = 0
if button(rect(100, 100, 80, 20), "Go") then n += 1 end
)petal";

TEST(ui_batches_and_retained_layers_are_lowered_by_default) {
    petal::Vm vm;
    vm.set_dimensions(400, 300);
    vm.load_source(RETAINED_UI);
    vm.begin_frame(0.016, 0, 0);
    vm.run();
    auto cmds = vm.drain_draw();
    REQUIRE(cmds.size() > 4);
    // The layer's content, moved to where it is drawn and clipped to its size.
    CHECK_EQ(cmds[0].kind, uint32_t(PB_DRAW_CLIP_PUSH));
    CHECK_EQ(cmds[0].w, 30);
    CHECK_EQ(cmds[1].kind, uint32_t(PB_DRAW_RECT));
    CHECK_EQ(cmds[1].x, 10);
    CHECK_EQ(cmds[1].y, 20);
    CHECK_EQ(cmds[2].kind, uint32_t(PB_DRAW_RECT));
    CHECK_EQ(int(cmds[2].color.a), 128);
    CHECK_EQ(cmds[3].kind, uint32_t(PB_DRAW_CLIP_POP));
}

TEST(ui_batches_and_retained_layers_reach_a_host_that_asks) {
    petal::Vm vm;
    vm.set_dimensions(400, 300);
    vm.load_source(RETAINED_UI);
    REQUIRE(pb_vm_set_draw_caps(vm.raw(), PB_DRAW_CAP_BATCH_RECTS | PB_DRAW_CAP_RETAINED_LAYERS) == PB_OK);
    vm.begin_frame(0.016, 0, 0);
    vm.run();
    auto cmds = vm.drain_draw();
    REQUIRE(cmds.size() > 4);
    CHECK_EQ(cmds[0].kind, uint32_t(PB_DRAW_CACHE_BEGIN));
    CHECK_EQ(cmds[0].w, 30);
    CHECK_EQ(cmds[0].h, 40);
    const uint32_t id = cmds[0].id;
    CHECK_EQ(cmds[1].kind, uint32_t(PB_DRAW_RECTS));
    REQUIRE(cmds[1].point_count == 2);
    CHECK_EQ(cmds[1].points[5], 4);                       // second rect's x, layer-local
    CHECK_EQ(uint32_t(cmds[1].points[4]), 0xff0000ffu);   // r << 24 | g << 16 | b << 8 | a
    CHECK_EQ(uint32_t(cmds[1].points[9]), 0x00ff0080u);
    CHECK_EQ(cmds[2].kind, uint32_t(PB_DRAW_CACHE_END));
    CHECK_EQ(cmds[3].kind, uint32_t(PB_DRAW_CACHE_DRAW));
    CHECK_EQ(cmds[3].id, id);
    CHECK_EQ(cmds[3].x, 10);
    CHECK_EQ(cmds[3].y, 20);

    // The next frame draws the layer the host kept, and records nothing.
    vm.key("z", true);
    vm.begin_frame(0.016, 1, 0.016);
    vm.run();
    auto next = vm.drain_draw();
    REQUIRE(!next.empty());
    CHECK_EQ(next[0].kind, uint32_t(PB_DRAW_CACHE_DRAW));

    // Until the host says it lost them.
    REQUIRE(pb_vm_invalidate_layers(vm.raw()) == PB_OK);
    vm.begin_frame(0.016, 2, 0.032);
    vm.run();
    auto again = vm.drain_draw();
    REQUIRE(!again.empty());
    CHECK_EQ(again[0].kind, uint32_t(PB_DRAW_CACHE_BEGIN));
}

TEST(ui_targets_are_readable_and_clickable_by_id) {
    petal::Vm vm;
    vm.set_dimensions(400, 300);
    vm.load_source(RETAINED_UI);
    vm.begin_frame(0.016, 0, 0);
    vm.run();

    pb_ui_target t;
    REQUIRE(pb_vm_ui_target(vm.raw(), "Go", &t) == 1);
    CHECK_EQ(std::string(t.id), std::string("Go"));
    CHECK_EQ(t.x, 100.0);
    CHECK_EQ(t.w, 80.0);
    CHECK_EQ(int(t.blocks), 1);
    CHECK_EQ(pb_vm_ui_target(vm.raw(), "Stop", &t), 0);

    const pb_ui_target* all = nullptr;
    size_t count = 0;
    REQUIRE(pb_vm_ui_targets(vm.raw(), &all, &count) == PB_OK);
    REQUIRE(count == 1);
    CHECK_EQ(std::string(all[0].id), std::string("Go"));

    // A scenario names the button instead of knowing where it is.
    petal::Scenario sc = petal::Scenario::from_json(R"({"events": [{"at": 1, "click_target": "Go"}]})");
    for (size_t f = 1; f < 4; ++f) {
        vm.apply_scenario(sc, f);
        vm.begin_frame(0.016, int64_t(f), f * 0.016);
        vm.run();
    }
    CHECK_EQ(vm.state("n").integer(), 1);
}

// ─── Tests written in the script ────────────────────────────────────────────

TEST(script_tests_are_listed_run_and_wait_for_frames) {
    petal::Vm vm;
    int wanted = 0;
    vm.native("wait_frames", petal::fx::Effect, [&](petal::Call& c) {
        if (!c.suspend()) return;
        wanted = int(c.arg(0).integer());
    });
    vm.native("expect_even", petal::fx::Effect, [](petal::Call& c) {
        auto at = c.location();
        c.expect(c.arg(0).integer() % 2 == 0, "odd at line " + std::to_string(at.line));
    });
    vm.load_source(R"(native fn wait_frames(n)
native fn expect_even(n)
state var ticks = 0
set ticks = ticks + 1
test "arithmetic" do
  expect_eq(1 + 1, 2)
  expect_eq(1 + 1, 3, "sum")
  expect_even(3)
end
test "two frames pass" do
  let before = get ticks
  wait_frames(2)
  expect_eq(get ticks, before + 2)
end
)");
    vm.run();
    std::string tests = vm.tests_json();
    CHECK_CONTAINS(tests, R"("name":"arithmetic")");
    CHECK_CONTAINS(tests, R"("function":"__test_1")");

    std::string r = vm.run_test("__test_0");
    CHECK_CONTAINS(r, R"("passed":false)");
    CHECK_CONTAINS(r, R"("checks":3)");
    CHECK_CONTAINS(r, R"("failed":2)");
    CHECK_CONTAINS(r, R"("note":"sum")");
    CHECK_CONTAINS(r, "odd at line 8");
    CHECK_EQ(vm.test_result_json(), r);

    vm.test_begin("__test_1");
    CHECK_EQ(vm.test_in_progress(), std::string("__test_1"));
    int frames = 0;
    for (;;) {
        pb_test_step step = vm.test_resume();
        if (step != PB_TEST_SUSPENDED) {
            CHECK_EQ(int(step), int(PB_TEST_FINISHED));
            break;
        }
        for (; wanted > 0; --wanted, ++frames) vm.run();
    }
    CHECK_EQ(frames, 2);
    CHECK_CONTAINS(vm.test_result_json(), R"("passed":true)");
    CHECK_EQ(vm.test_in_progress(), std::string(""));

    // Nothing in progress; and a failure outside a test is the host's to take.
    CHECK_EQ(pb_vm_test_resume(vm.raw(), 0), -1);
    CHECK_EQ(pb_vm_test_begin(vm.raw(), "missing"), PB_ERR_NOT_FOUND);
    vm.load_source("expect(1 > 2, \"never\")\n");
    vm.run();
    std::string taken = vm.take_expect_failures_json();
    CHECK_CONTAINS(taken, R"("failed":1)");
    CHECK_CONTAINS(taken, "never");
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
