// scene_frame.cpp — the C++ half of core/examples/scene_frame: the same
// game-shaped script (2,000 static city meshes, 400 static crate bodies, 300
// moving ships) run through the C bridge, with a host that reads `body` and
// `draw_mesh` options the way Cheesecake's script host does. It measures
// what L4 of the retained optimization plan changes at the bridge
// (docs/dev/retained-scene-output.md, "L4"):
//
//   lookup  today's reads: an allow-list check per call (O(fields x allowed)
//           string compares) and one linear key scan per option read
//   plan    pb_plan: the option names are checked once per record shape and
//           the values are read by position
//   diff    plan, and a keyed body compares its options with last frame's
//           (pb_snapshot) and reads and applies only the ones that changed
//
// each with the scene natives as host callbacks applied inside the call
// (Cheesecake today; `body` returns a state record, as Cheesecake's does) and
// as emitters drained after the run (L1), memo on. Like Cheesecake's natives,
// the host reads every option it supports on every call (40 for `body`, 20
// for `draw_mesh`), most of them absent. Every mode's world (bodies and
// draws, every option) is hashed every frame and must be the same.
//
//   build/petal_bridge_scene_frame [frames] [city] [crates] [ships]
//   MODES=callback.plan,emit.plan build/petal_bridge_scene_frame
//
// Not a test: the numbers depend on the machine.
#include <petal.hpp>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <memory>
#include <sstream>
#include <string>
#include <unordered_map>
#include <vector>

#ifndef PETAL_SCENE_FRAME_PTL
#define PETAL_SCENE_FRAME_PTL "core/examples/scene_frame/scene.ptl"
#endif

using clk = std::chrono::steady_clock;
static double us(clk::duration d) { return std::chrono::duration<double, std::micro>(d).count(); }

// ─── The mock engine (the same costs as the Rust mock in scene_frame) ───────

static const std::string_view kBodyAllowed[] = {
    "shape", "size", "radius", "height", "mass", "static", "kinematic", "pos", "rot", "vel", "spin",
    "restitution", "friction", "rolling_friction", "damping", "angular_damping", "gravity_scale",
    "sleep", "layer", "mask", "visible", "shadows", "mesh", "material", "impact_sound",
    "impact_volume", "color", "roughness", "metallic", "emissive", "opacity", "pattern",
    "pattern_scale", "texture", "shader", "params", "unlit", "double_sided", "tint", "orbit",
};
// The body options the mock applies, by their position in kBodyAllowed.
enum BodyOpt { B_SHAPE = 0, B_SIZE = 1, B_MASS = 4, B_STATIC = 5, B_KINEMATIC = 6, B_POS = 7,
               B_COLOR = 26, B_ROUGHNESS = 27, B_METALLIC = 28 };

static const std::string_view kDrawAllowed[] = {
    "pos", "rot", "scale", "material", "shadows", "max_distance", "min_distance", "color",
    "roughness", "metallic", "emissive", "opacity", "pattern", "pattern_scale", "texture", "shader",
    "params", "unlit", "double_sided", "tint",
};
enum DrawOpt { D_POS = 0, D_ROT = 1, D_SCALE = 2, D_COLOR = 7, D_ROUGHNESS = 8, D_METALLIC = 9,
               D_EMISSIVE = 10 };

template <size_t N>
static std::string plan_list(const std::string_view (&names)[N]) {
    std::string s;
    for (auto n : names) { if (!s.empty()) s += ", "; s += n; }
    return s;
}

// Cheesecake's check_options: every field must be one the native knows.
template <size_t N>
static void check_options(petal::Value v, const char* where, const std::string_view (&allowed)[N]) {
    if (v.is_nil()) return;
    if (!v.is_map()) throw std::runtime_error(std::string(where) + ": options must be a record");
    for (petal::Value f : v) {
        if (std::find(std::begin(allowed), std::end(allowed), f.key()) == std::end(allowed))
            throw std::runtime_error(std::string(where) + ": unknown option `" + std::string(f.key()) + "`");
    }
}

struct V3 { double x = 0, y = 0, z = 0; bool operator==(const V3&) const = default; };
static V3 v3(petal::Value v, V3 d = {}) { return v.is_vec3() ? V3{v.x(), v.y(), v.z()} : d; }

constexpr size_t kBodyN = std::size(kBodyAllowed);
constexpr size_t kDrawN = std::size(kDrawAllowed);

struct BodyDesc {
    std::string shape = "box";
    double size = 1, mass = 1;
    bool is_static = false, kinematic = false;
    double color = 0, roughness = 0.5, metallic = 0;
    V3 pos;
    // Every other option, read as a number (0 when absent): the reads a
    // real body() makes for the options this mock does not model.
    double rest[kBodyN] = {};
    bool operator==(const BodyDesc&) const = default;
};

static bool body_modeled(size_t i) {
    switch (i) {
        case B_SHAPE: case B_SIZE: case B_MASS: case B_STATIC: case B_KINEMATIC: case B_POS:
        case B_COLOR: case B_ROUGHNESS: case B_METALLIC: return true;
        default: return false;
    }
}
static bool draw_modeled(size_t i) {
    switch (i) {
        case D_POS: case D_ROT: case D_SCALE: case D_COLOR: case D_ROUGHNESS: case D_METALLIC: case D_EMISSIVE:
            return true;
        default: return false;
    }
}

// Option i of a body, read into its field of `d`.
static void apply_body(BodyDesc& d, size_t i, petal::Value v) {
    switch (i) {
        case B_SHAPE: d.shape = v.is_string() ? v.string() : "box"; break;
        case B_SIZE: d.size = v.number(1.0); break;
        case B_MASS: d.mass = v.number(1.0); break;
        case B_STATIC: d.is_static = v.boolean(); break;
        case B_KINEMATIC: d.kinematic = v.boolean(); break;
        case B_POS: d.pos = v3(v); break;
        case B_COLOR: d.color = v.number(0.0); break;
        case B_ROUGHNESS: d.roughness = v.number(0.5); break;
        case B_METALLIC: d.metallic = v.number(0.0); break;
        default: d.rest[i] = v.number(0.0); break;
    }
}

struct BodyEntry {
    BodyDesc desc;
    uint64_t declared = 0;
    petal::Snapshot snap;  // plan mode: last frame's option values
};

struct MeshDraw {
    std::string mesh;
    float xform[16];
    float material[36];
};

struct DrawFields {
    V3 pos, scale{1, 1, 1};
    double rot = 0, color = 0, roughness = 0.5, metallic = 0, emissive = 0;
    double rest[kDrawN] = {};
};

static MeshDraw compute_draw(std::string_view name, const DrawFields& f) {
    MeshDraw d{};
    d.mesh = std::string(name);
    float c = float(std::cos(f.rot)), s = float(std::sin(f.rot));
    d.xform[0] = c * float(f.scale.x);
    d.xform[2] = -s * float(f.scale.x);
    d.xform[5] = float(f.scale.y);
    d.xform[8] = s * float(f.scale.z);
    d.xform[10] = c * float(f.scale.z);
    d.xform[12] = float(f.pos.x);
    d.xform[13] = float(f.pos.y);
    d.xform[14] = float(f.pos.z);
    d.xform[15] = 1;
    d.material[0] = float(f.color);
    d.material[1] = float(f.roughness);
    d.material[2] = float(f.metallic);
    d.material[3] = float(f.emissive);
    for (size_t i = 0; i < kDrawN; ++i) d.material[4 + i] = float(f.rest[i]);
    return d;
}

enum class Reads { Lookup, Plan, Diff };

struct World {
    Reads reads = Reads::Lookup;
    uint64_t frame = 0;
    std::unordered_map<std::string, BodyEntry> bodies;
    std::vector<MeshDraw> draws;
    petal::Plan body_plan{plan_list(kBodyAllowed)};
    petal::Plan draw_plan{plan_list(kDrawAllowed)};
    uint64_t body_changes = 0, fields_applied = 0;

    void begin() { ++frame; draws.clear(); }

    static std::string id_key(petal::Value id) {
        if (id.is_string()) return id.string();
        return std::to_string(id.integer());
    }

    BodyEntry& entry(petal::Value id) {
        BodyEntry& e = bodies[id_key(id)];
        e.declared = frame;
        return e;
    }

    void body(petal::Value id, petal::Value opts) {
        if (reads == Reads::Lookup) {
            check_options(opts, "body", kBodyAllowed);
            BodyDesc d;
            if (opts["shape"].is_string()) d.shape = opts["shape"].string();
            d.size = opts.num("size", 1.0);
            d.mass = opts.num("mass", 1.0);
            d.is_static = opts["static"].boolean();
            d.kinematic = opts["kinematic"].boolean();
            d.color = opts.num("color", 0.0);
            d.roughness = opts.num("roughness", 0.5);
            d.metallic = opts.num("metallic", 0.0);
            d.pos = v3(opts["pos"]);
            for (size_t i = 0; i < kBodyN; ++i)
                if (!body_modeled(i)) d.rest[i] = opts.num(kBodyAllowed[i], 0.0);
            BodyEntry& e = entry(id);
            // Everything is parsed and applied whenever anything differs.
            if (!(e.desc == d)) { e.desc = d; ++body_changes; fields_applied += kBodyN; }
            return;
        }
        auto o = body_plan.bind(opts);
        if (!o.ok()) throw std::runtime_error("body: unknown option `" + std::string(o.unknown().key()) + "`");
        BodyEntry& e = entry(id);
        if (reads == Reads::Plan) {
            BodyDesc d;
            for (size_t i = 0; i < kBodyN; ++i) apply_body(d, i, o[i]);
            if (!(e.desc == d)) { e.desc = d; ++body_changes; fields_applied += kBodyN; }
            return;
        }
        // Only the options that differ from the ones this body was last
        // declared with (all of them the first time) are applied.
        auto changed = e.snap.update(o);
        if (!changed.any()) return;
        ++body_changes;
        for (size_t i : changed) {
            ++fields_applied;
            apply_body(e.desc, i, o[i]);
        }
    }

    void draw_mesh(petal::Value name, petal::Value opts) {
        DrawFields f;
        if (reads == Reads::Lookup) {
            check_options(opts, "draw_mesh", kDrawAllowed);
            f.pos = v3(opts["pos"]);
            f.scale = v3(opts["scale"], {1, 1, 1});
            f.rot = opts.num("rot", 0.0);
            f.color = opts.num("color", 0.0);
            f.roughness = opts.num("roughness", 0.5);
            f.metallic = opts.num("metallic", 0.0);
            f.emissive = opts.num("emissive", 0.0);
            for (size_t i = 0; i < kDrawN; ++i)
                if (!draw_modeled(i)) f.rest[i] = opts.num(kDrawAllowed[i], 0.0);
        } else {
            auto o = draw_plan.bind(opts);
            if (!o.ok()) throw std::runtime_error("draw_mesh: unknown option `" + std::string(o.unknown().key()) + "`");
            f.pos = v3(o[D_POS]);
            f.scale = v3(o[D_SCALE], {1, 1, 1});
            f.rot = o[D_ROT].number(0.0);
            f.color = o[D_COLOR].number(0.0);
            f.roughness = o[D_ROUGHNESS].number(0.5);
            f.metallic = o[D_METALLIC].number(0.0);
            f.emissive = o[D_EMISSIVE].number(0.0);
            for (size_t i = 0; i < kDrawN; ++i)
                if (!draw_modeled(i)) f.rest[i] = o[i].number(0.0);
        }
        draws.push_back(compute_draw(name.str(), f));
    }

    void end() {
        std::erase_if(bodies, [&](const auto& kv) { return kv.second.declared != frame; });
    }

    uint64_t digest() const {
        uint64_t h = bodies.size();
        auto mix = [&](uint64_t x) { h = h * 31 + x; };
        auto bits = [](double d) { uint64_t u; std::memcpy(&u, &d, 8); return u; };
        auto fbits = [](float f) { uint32_t u; std::memcpy(&u, &f, 4); return uint64_t(u); };
        std::vector<const std::pair<const std::string, BodyEntry>*> keys;
        for (auto& kv : bodies) keys.push_back(&kv);
        std::sort(keys.begin(), keys.end(), [](auto* a, auto* b) { return a->first < b->first; });
        for (auto* kv : keys) {
            for (char c : kv->first) mix(uint8_t(c));
            const BodyDesc& d = kv->second.desc;
            mix(bits(d.pos.x)); mix(bits(d.pos.y)); mix(bits(d.pos.z));
            mix(bits(d.size)); mix(bits(d.mass)); mix(bits(d.color)); mix(d.is_static); mix(d.kinematic);
            for (double r : d.rest) mix(bits(r));
            for (char c : d.shape) mix(uint8_t(c));
        }
        for (const MeshDraw& d : draws) {
            for (char c : d.mesh) mix(uint8_t(c));
            for (float x : d.xform) mix(fbits(x));
            for (float x : d.material) mix(fbits(x));
        }
        return h;
    }
};

// What Cheesecake's body() hands back: a fresh state record per call.
static void build_state(petal::BuilderRef b, const BodyEntry& e) {
    b.map([&](petal::BuilderRef m) {
        m.field("id", int64_t(0));
        m.key("pos").vec3(e.desc.pos.x, e.desc.pos.y, e.desc.pos.z);
        m.key("rot").map([&](petal::BuilderRef r) { r.field("x", 0.0).field("y", 0.0).field("z", 0.0).field("w", 1.0); });
        m.key("vel").vec3(0, 0, 0);
        m.key("spin").vec3(0, 0, 0);
        m.field("sleeping", false);
        m.field("impact", 0.0);
        m.field("mass", e.desc.mass);
    });
}

struct Mode {
    std::string name;
    bool emit;
    Reads reads;
};

struct Result {
    std::string name;
    double script_us, host_us, total_us, drain_us;
    double changes, fields;
    std::vector<uint64_t> digests;
};

static double median(std::vector<double> v) {
    std::sort(v.begin(), v.end());
    return v[v.size() / 2];
}

static Result run(const Mode& m, const std::string& source, int frames) {
    World w;
    w.reads = m.reads;
    petal::Vm vm;
    vm.set_echo(false);
    if (m.emit) {
        vm.emitter("body", "scene");
        vm.emitter("draw_mesh", "scene");
    } else {
        vm.native("body", petal::fx::Effect, [&](petal::Call& c) {
            w.body(c[0], c[1]);
            build_state(c.result(), w.bodies[World::id_key(c[0])]);
        });
        vm.native("draw_mesh", petal::fx::Effect, [&](petal::Call& c) { w.draw_mesh(c[0], c[1]); });
    }
    vm.load_source(source, "scene.ptl");
    std::vector<double> script, host, total, drain;
    std::vector<uint64_t> digests;
    const int warm = 30;
    for (int f = 0; f < frames + warm; ++f) {
        if (f == warm) w.body_changes = w.fields_applied = 0;
        w.begin();
        auto t0 = clk::now();
        vm.begin_frame(1.0 / 60.0, f, f / 60.0);
        vm.run();
        auto t1 = clk::now();
        auto td = t1;
        if (m.emit) {
            petal::Values cmds = vm.drain("scene");
            td = clk::now();
            for (petal::Value cmd : cmds) {
                if (cmd.tag() == "body") w.body(cmd[0], cmd[1]);
                else w.draw_mesh(cmd[0], cmd[1]);
            }
        }
        w.end();
        auto t2 = clk::now();
        if (f >= warm) {
            script.push_back(us(t1 - t0));
            host.push_back(us(t2 - t1));
            total.push_back(us(t2 - t0));
            drain.push_back(us(td - t1));
            digests.push_back(w.digest());
        }
    }
    return {m.name, median(script), median(host), median(total), median(drain),
            double(w.body_changes) / frames, double(w.fields_applied) / frames, std::move(digests)};
}

static std::string replace(std::string s, const std::string& from, const std::string& to) {
    auto at = s.find(from);
    if (at == std::string::npos) { std::fprintf(stderr, "scene.ptl lacks `%s`\n", from.c_str()); std::exit(1); }
    return s.replace(at, from.size(), to);
}

int main(int argc, char** argv) {
    auto arg = [&](int i, int d) { return argc > i ? std::atoi(argv[i]) : d; };
    int frames = arg(1, 300), city = arg(2, 2000), crates = arg(3, 400), ships = arg(4, 300);
    std::ifstream in(PETAL_SCENE_FRAME_PTL);
    if (!in) { std::fprintf(stderr, "cannot read %s\n", PETAL_SCENE_FRAME_PTL); return 1; }
    std::stringstream ss;
    ss << in.rdbuf();
    std::string source = ss.str();
    source = replace(source, "const N_CITY = 2000", "const N_CITY = " + std::to_string(city));
    source = replace(source, "const N_CRATES = 400", "const N_CRATES = " + std::to_string(crates));
    source = replace(source, "const N_SHIPS = 300", "const N_SHIPS = " + std::to_string(ships));
    std::printf("world: %d static meshes, %d static bodies, %d moving ships (body + mesh); %d frames, median\n",
                city, crates, ships, frames);

    std::vector<Mode> modes = {
        {"callback.lookup", false, Reads::Lookup},
        {"callback.plan", false, Reads::Plan},
        {"callback.diff", false, Reads::Diff},
        {"emit.lookup", true, Reads::Lookup},
        {"emit.plan", true, Reads::Plan},
        {"emit.diff", true, Reads::Diff},
    };
    const char* only = std::getenv("MODES");
    std::vector<Result> results;
    for (const Mode& m : modes) {
        if (only && std::string(",") .append(only).append(",").find("," + m.name + ",") == std::string::npos) continue;
        results.push_back(run(m, source, frames));
    }
    // In callback modes the host's reads happen inside the script's calls,
    // so they count as script time.
    std::printf("%-18s %10s %10s %10s %10s %12s %12s\n", "mode", "script µs", "drain µs", "host µs", "total µs",
                "bodies/fr", "applied/fr");
    for (const Result& r : results)
        std::printf("%-18s %10.0f %10.0f %10.0f %10.0f %12.0f %12.0f\n", r.name.c_str(), r.script_us, r.drain_us,
                    r.host_us, r.total_us, r.changes, r.fields);
    bool same = std::all_of(results.begin(), results.end(), [&](const Result& r) { return r.digests == results[0].digests; });
    std::printf("worlds identical across modes, every frame: %s\n", same ? "true" : "false");
    return same ? 0 : 1;
}
