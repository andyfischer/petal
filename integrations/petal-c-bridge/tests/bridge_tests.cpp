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
    CHECK(vm.set_config("ROWS[0][1]", "{\"int\": 3}"));
    CHECK(vm.last_reload_json().find("\"rebuilt_names\":[\"ROWS\"]") != std::string::npos);
    CHECK_EQ(run_out(vm), std::vector<double>{302});
    // The second waits: the table keeps the value of the tick before.
    CHECK(vm.set_config("ROWS[0][1]", "{\"int\": 4}"));
    CHECK(vm.last_reload_json().find("\"deferred\":[\"ROWS\"]") != std::string::npos);
    CHECK_EQ(run_out(vm), std::vector<double>{303});
    // The host knows the drag ended.
    CHECK_EQ(vm.flush_static_rebuilds(), 1);
    CHECK_EQ(vm.flush_static_rebuilds(), 0);
    CHECK_EQ(run_out(vm), std::vector<double>{404});
    // Off: every tick rebuilds. And a reload that follows nothing reports
    // nothing about constants.
    vm.set_static_debounce(-1, 0);
    CHECK(vm.set_config("ROWS[0][1]", "{\"int\": 5}"));
    CHECK(vm.set_config("ROWS[0][1]", "{\"int\": 6}"));
    CHECK(vm.last_reload_json().find("\"deferred\":[]") != std::string::npos);
    CHECK_EQ(run_out(vm), std::vector<double>{605});
    vm.set_static_follow(false);
    CHECK(vm.set_config("ROWS[0][1]", "{\"int\": 7}"));
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
    "let counts = map([\"a\", \"b\"], fn(k) bump(k) end)\n"
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
    CHECK_CONTAINS(a.state_report_json(), R"({"holds":"a function","name":"on_done"})");
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
    CHECK_CONTAINS(vm.last_reload_json(), R"("new_diagnostics":[{"column":)");

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
    CHECK(vm.set_config("POST.steps[1]", "{\"int\":9}"));
    CHECK(vm.set_config("POST.bloom", "0.75", config.string()));
    CHECK(!vm.sources_changed());
    CHECK_EQ(vm.last_reload_kind(), PB_RELOAD_PATCHED);
    CHECK_EQ(run_out(vm), std::vector<double>{84});
    // A value of another type needs the file and a reload.
    CHECK(!vm.set_config("POST.steps[1]", "9.5"));
    CHECK_THROWS_CODE(vm.set_config("POST.missing.deep", "1"), PB_ERR_NOT_FOUND);
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
