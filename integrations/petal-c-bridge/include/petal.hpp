// petal.hpp — header-only C++20 wrapper over petal_bridge.h.
//
// Error policy: every failing operation throws petal::Error (which carries the
// status code, phase and per-diagnostic file/line/column). There is no
// expected-style variant; a frame loop wraps run()/reload() in try/catch.
//
// Ownership: petal::Vm owns the pb_vm (move-only RAII). petal::Value /
// petal::Values are non-owning views into bridge memory whose lifetime ends
// at the next run()/call()/load*()/reload*()/clear_views() on the same Vm —
// copy out what you need to keep. petal::Builder owns a pb_builder.
//
//   petal::Vm vm;
//   vm.native("raycast", petal::fx::ReadsHostData, [&](petal::Call& c) {
//       auto hit = world.raycast(c[0].num("x"), ...);
//       c.result().map([&](petal::BuilderRef m) { m.field("dist", hit.dist); });
//   });
//   vm.emitter("spawn", "scene");           // spawn(...) -> "scene" buffer
//   vm.load_file("game.ptl");
//   vm.begin_frame(dt, frame, t);
//   vm.run();
//   for (petal::Value cmd : vm.drain("scene")) { cmd.tag(); cmd[0]; ... }
//   for (const pb_draw_cmd& d : vm.drain_draw()) { ... }
#pragma once

#include "petal_bridge.h"

#include <concepts>
#include <cstdint>
#include <functional>
#include <optional>
#include <span>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace petal {

// ─── Errors ─────────────────────────────────────────────────────────────────

struct ErrorItem {
    std::string message;
    std::string file;
    uint32_t line = 0;    // 1-based; 0 = unknown
    uint32_t column = 0;  // 1-based; 0 = unknown
};

class Error : public std::runtime_error {
public:
    Error(pb_status code, std::string message, std::string phase = {}, std::vector<ErrorItem> items = {})
        : std::runtime_error(std::move(message)), code_(code), phase_(std::move(phase)), items_(std::move(items)) {}

    pb_status code() const noexcept { return code_; }
    // "lex", "parse", "module", "compile", "lower", "runtime" or "".
    const std::string& phase() const noexcept { return phase_; }
    const std::vector<ErrorItem>& items() const noexcept { return items_; }
    // Position of the first diagnostic (empty / 0 when unknown).
    std::string file() const { return items_.empty() ? std::string{} : items_[0].file; }
    uint32_t line() const noexcept { return items_.empty() ? 0 : items_[0].line; }
    uint32_t column() const noexcept { return items_.empty() ? 0 : items_[0].column; }

    bool is_compile_error() const noexcept { return code_ == PB_ERR_COMPILE; }
    bool is_runtime_error() const noexcept { return code_ == PB_ERR_RUNTIME; }

    // Build from a VM's last error (falls back to the status name).
    static Error from_vm(const pb_vm* vm, pb_status code) {
        const pb_error* e = vm ? pb_vm_last_error(vm) : nullptr;
        if (!e) return Error(code, pb_status_name(code));
        std::vector<ErrorItem> items;
        items.reserve(e->item_count);
        for (size_t i = 0; i < e->item_count; ++i) {
            const pb_error_item& it = e->items[i];
            items.push_back({it.message ? it.message : "", it.file ? it.file : "", it.line, it.column});
        }
        return Error(e->code, e->message ? e->message : "", e->phase ? e->phase : "", std::move(items));
    }

private:
    pb_status code_;
    std::string phase_;
    std::vector<ErrorItem> items_;
};

namespace detail {
inline void check(const pb_vm* vm, pb_status st) {
    if (st != PB_OK) throw Error::from_vm(vm, st);
}
}  // namespace detail

// ─── Native effect flags ────────────────────────────────────────────────────

namespace fx {
inline constexpr uint32_t Pure = PB_FX_PURE;
inline constexpr uint32_t Probe = PB_FX_PROBE;
inline constexpr uint32_t Emits = PB_FX_EMITS;
inline constexpr uint32_t Effect = PB_FX_EFFECT;
inline constexpr uint32_t PendingEffectful = PB_FX_PENDING_EFFECTFUL;
inline constexpr uint32_t PendingAllow = PB_FX_PENDING_ALLOW;
inline constexpr uint32_t ReadsPointer = PB_FX_READS_POINTER;
inline constexpr uint32_t ReadsKeyboard = PB_FX_READS_KEYBOARD;
inline constexpr uint32_t ReadsClock = PB_FX_READS_CLOCK;
inline constexpr uint32_t ReadsViewport = PB_FX_READS_VIEWPORT;
inline constexpr uint32_t ReadsHostData = PB_FX_READS_HOST_DATA;
inline constexpr uint32_t ReadsResources = PB_FX_READS_RESOURCES;
inline constexpr uint32_t ReadsRng = PB_FX_READS_RNG;
inline constexpr uint32_t ReadsBindings = PB_FX_READS_BINDINGS;
}  // namespace fx

// ─── Value views ────────────────────────────────────────────────────────────

// A non-owning view of a decoded Petal value. A default/missing Value is nil,
// so lookups chain safely: v["pos"]["x"].number().
class Value {
public:
    Value() = default;
    explicit Value(const pb_value* v) : v_(v) {}

    const pb_value* raw() const noexcept { return v_; }
    pb_kind kind() const noexcept { return v_ ? static_cast<pb_kind>(v_->kind) : PB_NIL; }

    bool is_nil() const noexcept { return kind() == PB_NIL; }
    bool is_bool() const noexcept { return kind() == PB_BOOL; }
    bool is_int() const noexcept { return kind() == PB_INT; }
    bool is_float() const noexcept { return kind() == PB_FLOAT; }
    bool is_number() const noexcept { return pb_value_is_number(v_); }
    bool is_vec2() const noexcept { return kind() == PB_VEC2; }
    bool is_string() const noexcept { return kind() == PB_STRING; }
    bool is_list() const noexcept { return kind() == PB_LIST; }
    bool is_map() const noexcept { return kind() == PB_MAP; }
    bool is_enum() const noexcept { return kind() == PB_ENUM; }
    bool is_symbol() const noexcept { return kind() == PB_SYMBOL; }
    bool is_handle() const noexcept { return kind() == PB_HANDLE; }

    // INT/FLOAT/BOOL as a double, else `fallback`.
    double number(double fallback = 0.0) const noexcept { return pb_value_num(v_, fallback); }
    // INT/FLOAT/BOOL as an integer (floats truncate), else `fallback`.
    int64_t integer(int64_t fallback = 0) const noexcept { return is_number() ? v_->integer : fallback; }
    // Truthiness of BOOL (and nonzero numbers); false for everything else.
    bool boolean(bool fallback = false) const noexcept { return is_number() ? v_->integer != 0 : fallback; }
    // VEC2 components (number = x, y = y).
    double x() const noexcept { return is_vec2() ? v_->number : 0.0; }
    double y() const noexcept { return is_vec2() ? v_->y : 0.0; }

    // STRING text, ENUM tag, SYMBOL name, MAP class name, HANDLE class, OTHER type name.
    std::string_view str() const noexcept {
        return (v_ && v_->str) ? std::string_view(v_->str, v_->str_len) : std::string_view{};
    }
    std::string string() const { return std::string(str()); }
    // ENUM variant tag ("" if not an enum).
    std::string_view tag() const noexcept { return is_enum() ? str() : std::string_view{}; }
    // Field name when this value is a map entry.
    std::string_view key() const noexcept {
        return (v_ && v_->key) ? std::string_view(v_->key, v_->key_len) : std::string_view{};
    }

    // Children of LIST / MAP / ENUM.
    size_t size() const noexcept { return (is_list() || is_map() || is_enum()) ? v_->count : 0; }
    bool empty() const noexcept { return size() == 0; }
    Value operator[](size_t i) const noexcept { return Value(pb_value_at(v_, i)); }
    Value operator[](std::string_view field) const noexcept { return get(field); }
    Value get(std::string_view field) const noexcept {
        if (!is_map()) return Value();
        for (uint32_t i = 0; i < v_->count; ++i) {
            const pb_value* f = &v_->items[i];
            if (std::string_view(f->key, f->key_len) == field) return Value(f);
        }
        return Value();
    }
    bool has(std::string_view field) const noexcept { return get(field).raw() != nullptr; }
    // Numeric field of a map, or `fallback`.
    double num(std::string_view field, double fallback = 0.0) const noexcept { return get(field).number(fallback); }

    class iterator {
    public:
        using value_type = Value;
        using difference_type = std::ptrdiff_t;
        iterator() = default;
        explicit iterator(const pb_value* p) : p_(p) {}
        Value operator*() const noexcept { return Value(p_); }
        iterator& operator++() noexcept { ++p_; return *this; }
        iterator operator++(int) noexcept { auto t = *this; ++p_; return t; }
        bool operator==(const iterator&) const = default;
    private:
        const pb_value* p_ = nullptr;
    };
    iterator begin() const noexcept { return iterator(v_ ? v_->items : nullptr); }
    iterator end() const noexcept { return iterator(v_ && v_->items ? v_->items + v_->count : nullptr); }

private:
    const pb_value* v_ = nullptr;
};

// A contiguous run of root values (a drained buffer).
class Values {
public:
    Values() = default;
    explicit Values(pb_values v) : v_(v) {}
    size_t size() const noexcept { return v_.count; }
    bool empty() const noexcept { return v_.count == 0; }
    Value operator[](size_t i) const noexcept { return i < v_.count ? Value(&v_.items[i]) : Value(); }
    Value back() const noexcept { return empty() ? Value() : (*this)[size() - 1]; }
    Value::iterator begin() const noexcept { return Value::iterator(v_.items); }
    Value::iterator end() const noexcept { return Value::iterator(v_.items ? v_.items + v_.count : nullptr); }

private:
    pb_values v_{nullptr, 0};
};

// ─── Builders ───────────────────────────────────────────────────────────────

// Non-owning fluent interface over a pb_builder. Each value method appends
// one value to the innermost open container (or to the roots).
class BuilderRef {
public:
    explicit BuilderRef(pb_builder* b) : b_(b) {}
    pb_builder* raw() const noexcept { return b_; }

    BuilderRef& nil() { pb_builder_nil(b_); return *this; }
    BuilderRef& value(bool v) { pb_builder_bool(b_, v); return *this; }
    BuilderRef& value(int v) { pb_builder_int(b_, v); return *this; }
    BuilderRef& value(int64_t v) { pb_builder_int(b_, v); return *this; }
    BuilderRef& value(double v) { pb_builder_float(b_, v); return *this; }
    BuilderRef& value(float v) { pb_builder_float(b_, v); return *this; }
    BuilderRef& value(std::string_view s) { pb_builder_string(b_, s.data(), s.size()); return *this; }
    BuilderRef& value(const char* s) { return value(std::string_view(s)); }
    BuilderRef& value(const std::string& s) { return value(std::string_view(s)); }
    BuilderRef& vec2(double x, double y) { pb_builder_vec2(b_, x, y); return *this; }
    BuilderRef& vec3(double x, double y, double z) { pb_builder_vec3(b_, x, y, z); return *this; }
    BuilderRef& symbol(const std::string& name) { pb_builder_symbol(b_, name.c_str()); return *this; }
    BuilderRef& floats(std::span<const double> v) { pb_builder_floats(b_, v.data(), v.size()); return *this; }

    BuilderRef& begin_list() { pb_builder_begin_list(b_); return *this; }
    BuilderRef& end_list() { pb_builder_end_list(b_); return *this; }
    BuilderRef& begin_map() { pb_builder_begin_map(b_); return *this; }
    BuilderRef& end_map() { pb_builder_end_map(b_); return *this; }
    BuilderRef& begin_enum(const std::string& tag) { pb_builder_begin_enum(b_, tag.c_str()); return *this; }
    BuilderRef& end_enum() { pb_builder_end_enum(b_); return *this; }
    BuilderRef& key(const std::string& k) { pb_builder_key(b_, k.c_str()); return *this; }

    // Scoped forms: the callback fills the container.
    template <class F> BuilderRef& list(F&& fill) { begin_list(); fill(*this); return end_list(); }
    template <class F> BuilderRef& map(F&& fill) { begin_map(); fill(*this); return end_map(); }
    template <class F> BuilderRef& enum_(const std::string& tag, F&& fill) {
        begin_enum(tag); fill(*this); return end_enum();
    }
    // key(k) + value(v), inside a map.
    template <class T> BuilderRef& field(const std::string& k, T&& v) { key(k); return value(std::forward<T>(v)); }

    size_t root_count() const noexcept { return pb_builder_root_count(b_); }
    void clear() { pb_builder_clear(b_); }

private:
    pb_builder* b_;
};

// Owning builder.
class Builder : public BuilderRef {
public:
    Builder() : BuilderRef(pb_builder_new()) {
        if (!raw()) throw Error(PB_ERR_PANIC, "pb_builder_new failed");
    }
    ~Builder() { pb_builder_free(raw()); }
    Builder(const Builder&) = delete;
    Builder& operator=(const Builder&) = delete;
};

// ─── Host natives ───────────────────────────────────────────────────────────

// One in-flight native call. Arguments are views valid during the callback.
class Call {
public:
    explicit Call(pb_call* c) : c_(c) {}
    std::string_view name() const noexcept { return pb_call_name(c_); }
    size_t size() const noexcept { return pb_call_arg_count(c_); }
    Value arg(size_t i) const noexcept { return Value(pb_call_arg(c_, i)); }
    Value operator[](size_t i) const noexcept { return arg(i); }
    // Build at most one value here; leave it empty to return nil.
    BuilderRef result() const noexcept { return BuilderRef(pb_call_result(c_)); }
    pb_call* raw() const noexcept { return c_; }

private:
    pb_call* c_;
};

// A native body. Throw (any std::exception) to fail the script call.
using NativeFn = std::function<void(Call&)>;

struct ReloadResult {
    uint32_t state_preserved = 0;
    uint32_t state_dropped = 0;
};

// ─── Input scenarios ────────────────────────────────────────────────────────

// petal-ui's JSON input replay (events keyed by frame). Owning, move-only,
// independent of any Vm. Replay with Vm::apply_scenario(sc, frame) before
// begin_frame for that frame.
class Scenario {
public:
    Scenario() : s_(pb_scenario_new()) {
        if (!s_) throw Error(PB_ERR_PANIC, "pb_scenario_new failed");
    }
    ~Scenario() { if (s_) pb_scenario_free(s_); }
    Scenario(Scenario&& o) noexcept : s_(std::exchange(o.s_, nullptr)) {}
    Scenario& operator=(Scenario&& o) noexcept {
        if (this != &o) { if (s_) pb_scenario_free(s_); s_ = std::exchange(o.s_, nullptr); }
        return *this;
    }
    Scenario(const Scenario&) = delete;
    Scenario& operator=(const Scenario&) = delete;

    // Throw petal::Error (PB_ERR_INVALID_ARG / PB_ERR_IO) on a bad scenario.
    static Scenario from_json(const std::string& json) {
        Scenario sc;
        sc.check(pb_scenario_load_json(sc.s_, json.c_str()));
        return sc;
    }
    static Scenario from_file(const std::string& path) {
        Scenario sc;
        sc.check(pb_scenario_load_file(sc.s_, path.c_str()));
        return sc;
    }
    // Deterministic pseudo-random clicks/keys/text over `frames` frames.
    static Scenario monkey(uint64_t seed, size_t frames, int32_t width, int32_t height) {
        Scenario sc;
        sc.check(pb_scenario_monkey(sc.s_, seed, frames, width, height));
        return sc;
    }

    pb_scenario* raw() const noexcept { return s_; }
    size_t event_count() const noexcept { return pb_scenario_event_count(s_); }
    // One past the frame of the last event (0 = no events).
    size_t end_frame() const noexcept { return pb_scenario_end_frame(s_); }
    // The "frames" field, if present.
    std::optional<size_t> frames() const noexcept {
        size_t n = 0;
        if (pb_scenario_frames(s_, &n)) return n;
        return std::nullopt;
    }
    // The "size" field (width, height), if present.
    std::optional<std::pair<int32_t, int32_t>> size() const noexcept {
        int32_t w = 0, h = 0;
        if (pb_scenario_size(s_, &w, &h)) return std::pair{w, h};
        return std::nullopt;
    }
    std::string to_json() const {
        const char* j = pb_scenario_to_json(s_);
        return j ? j : "";
    }

private:
    void check(pb_status st) const {
        if (st == PB_OK) return;
        const char* e = pb_scenario_error(s_);
        throw Error(st, e ? e : pb_status_name(st));
    }

    pb_scenario* s_;
};

// ─── The VM ─────────────────────────────────────────────────────────────────

class Vm {
public:
    Vm() : vm_(pb_vm_create()) {
        if (!vm_) throw Error(PB_ERR_PANIC, "pb_vm_create failed");
        if (!pb_abi_check(sizeof(pb_value), sizeof(pb_draw_cmd), sizeof(pb_error))) {
            pb_vm_destroy(vm_);
            throw Error(PB_ERR_INVALID_ARG, "petal_bridge.h does not match the linked petal-bridge library");
        }
    }
    ~Vm() { if (vm_) pb_vm_destroy(vm_); }
    Vm(Vm&& o) noexcept : vm_(std::exchange(o.vm_, nullptr)) {}
    Vm& operator=(Vm&& o) noexcept {
        if (this != &o) { if (vm_) pb_vm_destroy(vm_); vm_ = std::exchange(o.vm_, nullptr); }
        return *this;
    }
    Vm(const Vm&) = delete;
    Vm& operator=(const Vm&) = delete;

    pb_vm* raw() const noexcept { return vm_; }

    // ── Modules ──
    void set_echo(bool on) { check(pb_vm_set_echo(vm_, on)); }
    void add_module_path(const std::string& dir) { check(pb_vm_add_module_path(vm_, dir.c_str())); }
    void register_module(const std::string& name, const std::string& source) {
        check(pb_vm_register_module(vm_, name.c_str(), source.c_str()));
    }
    // Returns the package name (e.g. "bloom").
    std::string add_package(const std::string& root) {
        const char* name = nullptr;
        check(pb_vm_add_package(vm_, root.c_str(), &name));
        return name ? name : "";
    }
    void add_implicit_import(const std::string& module) {
        check(pb_vm_add_implicit_import(vm_, module.c_str()));
    }

    // ── Host natives (register before the load that uses them) ──
    void native(const std::string& name, uint32_t effects, NativeFn fn) {
        auto* boxed = new NativeFn(std::move(fn));
        // On failure the bridge frees `boxed` through free_native.
        check(pb_vm_register_native(vm_, name.c_str(), &Vm::invoke_native, boxed, &Vm::free_native, effects));
    }
    void native(const std::string& name, NativeFn fn) { native(name, fx::Pure, std::move(fn)); }
    // name(args...) pushes tag(args...) into `buffer`; tag defaults to name.
    void emitter(const std::string& name, const std::string& buffer, const std::string& tag = {}) {
        check(pb_vm_register_emitter(vm_, name.c_str(), buffer.c_str(), tag.empty() ? nullptr : tag.c_str()));
    }

    // ── Loading ──
    void load_file(const std::string& path) { check(pb_vm_load_file(vm_, path.c_str())); }
    void load_source(const std::string& source, const std::string& name = "<source>") {
        check(pb_vm_load_source(vm_, source.c_str(), name.c_str()));
    }
    bool loaded() const noexcept { return pb_vm_is_loaded(vm_); }

    // ── Bindings (script: binding(symbol("name"))) ──
    void set_float(const std::string& n, double v) { check(pb_vm_set_float(vm_, n.c_str(), v)); }
    void set_int(const std::string& n, int64_t v) { check(pb_vm_set_int(vm_, n.c_str(), v)); }
    void set_bool(const std::string& n, bool v) { check(pb_vm_set_bool(vm_, n.c_str(), v)); }
    void set_string(const std::string& n, const std::string& v) { check(pb_vm_set_string(vm_, n.c_str(), v.c_str())); }
    void set_vec2(const std::string& n, double x, double y) { check(pb_vm_set_vec2(vm_, n.c_str(), x, y)); }
    void set_vec3(const std::string& n, double x, double y, double z) {
        check(pb_vm_set_vec3(vm_, n.c_str(), x, y, z));
    }
    void set_floats(const std::string& n, std::span<const double> v) {
        check(pb_vm_set_floats(vm_, n.c_str(), v.data(), v.size()));
    }
    // Binds the builder's single root value.
    void set_value(const std::string& n, const BuilderRef& b) { check(pb_vm_set_value(vm_, n.c_str(), b.raw())); }
    // Build and bind in one step: vm.set_value("snap", [](BuilderRef b){ b.map(...); });
    template <class F>
        requires std::invocable<F, BuilderRef&>
    void set_value(const std::string& n, F&& fill) {
        Builder b;
        BuilderRef& r = b;
        fill(r);
        set_value(n, b);
    }
    void clear_binding(const std::string& n) { check(pb_vm_clear_binding(vm_, n.c_str())); }

    // ── petal-ui input ──
    void mouse_move(int32_t x, int32_t y) { check(pb_vm_input_mouse_move(vm_, x, y)); }
    void mouse_motion(int32_t dx, int32_t dy) { check(pb_vm_input_mouse_motion(vm_, dx, dy)); }
    void mouse_button(uint8_t button, bool down) { check(pb_vm_input_mouse_button(vm_, button, down)); }
    void scroll(double dx, double dy) { check(pb_vm_input_scroll(vm_, dx, dy)); }
    // Throws PB_ERR_INVALID_ARG for a non-canonical key name.
    void key(const std::string& name, bool down) { check(pb_vm_input_key(vm_, name.c_str(), down)); }
    void text(const std::string& utf8) { check(pb_vm_input_text(vm_, utf8.c_str())); }
    void modifiers(uint32_t bits) { check(pb_vm_input_modifiers(vm_, bits)); }
    static bool is_canonical_key(const std::string& name) { return pb_key_is_canonical(name.c_str()); }

    void begin_frame(double dt, int64_t frame, double time_seconds) {
        check(pb_vm_begin_frame(vm_, dt, frame, time_seconds));
    }
    void set_dimensions(int32_t w, int32_t h) { check(pb_vm_set_dimensions(vm_, w, h)); }
    void set_text_metrics(double advance_ratio) { check(pb_vm_set_text_metrics(vm_, advance_ratio)); }
    void set_text_vertical_metrics(double baseline, double descent, double line_height, double cap_height,
                                   double x_height) {
        check(pb_vm_set_text_vertical_metrics(vm_, baseline, descent, line_height, cap_height, x_height));
    }
    void set_seed(uint64_t seed) { check(pb_vm_set_seed(vm_, seed)); }
    // Feed the events `sc` schedules for `frame`; call before begin_frame.
    // Returns how many were applied.
    size_t apply_scenario(const Scenario& sc, size_t frame) {
        size_t n = 0;
        check(pb_vm_apply_scenario(vm_, sc.raw(), frame, &n));
        return n;
    }

    // ── Running ──
    void run() { check(pb_vm_run(vm_)); }
    Value call(const std::string& function) { return call_raw(function, nullptr); }
    Value call(const std::string& function, const BuilderRef& args) { return call_raw(function, args.raw()); }
    // Whether the last run defined a top-level function `function`.
    bool has_function(const std::string& function) { return pb_vm_has_function(vm_, function.c_str()); }
    void clear_views() { pb_vm_clear_views(vm_); }

    // ── Output ──
    Values drain(const std::string& buffer) {
        pb_values out{nullptr, 0};
        check(pb_vm_drain(vm_, buffer.c_str(), &out));
        return Values(out);
    }
    std::span<const pb_draw_cmd> drain_draw() {
        const pb_draw_cmd* cmds = nullptr;
        size_t n = 0;
        check(pb_vm_drain_draw(vm_, &cmds, &n));
        return {cmds, n};
    }
    // true = grab, false = release, nullopt = no request this frame.
    std::optional<bool> take_mouse_grab() {
        int r = pb_vm_take_mouse_grab(vm_);
        if (r < 0) return std::nullopt;
        return r == 1;
    }

    // ── Hot reload ──
    std::vector<std::string> source_files() {
        const char* const* paths = nullptr;
        size_t n = 0;
        check(pb_vm_source_files(vm_, &paths, &n));
        return std::vector<std::string>(paths, paths + n);
    }
    bool sources_changed() { return pb_vm_sources_changed(vm_); }
    // Throws on compile error; the old program keeps running.
    ReloadResult reload() {
        pb_reload_result r{};
        check(pb_vm_reload(vm_, &r));
        return {r.state_preserved, r.state_dropped};
    }
    ReloadResult reload_source(const std::string& source) {
        pb_reload_result r{};
        check(pb_vm_reload_source(vm_, source.c_str(), &r));
        return {r.state_preserved, r.state_dropped};
    }

    // ── State / output tooling ──
    std::string state_json() {
        const char* s = pb_vm_state_json(vm_);
        if (!s) throw Error::from_vm(vm_, PB_ERR_NOT_LOADED);
        return s;
    }
    // A top-level state variable (throws PB_ERR_NOT_FOUND if absent).
    Value state(const std::string& name) {
        const pb_value* v = nullptr;
        check(pb_vm_get_state(vm_, name.c_str(), &v));
        return Value(v);
    }
    std::vector<std::string> take_output() {
        const char* const* lines = nullptr;
        size_t n = 0;
        check(pb_vm_take_output(vm_, &lines, &n));
        return std::vector<std::string>(lines, lines + n);
    }

private:
    void check(pb_status st) const { detail::check(vm_, st); }

    Value call_raw(const std::string& function, const pb_builder* args) {
        const pb_value* out = nullptr;
        check(pb_vm_call(vm_, function.c_str(), args, &out));
        return Value(out);
    }

    static int invoke_native(pb_call* call, void* userdata) noexcept {
        try {
            Call c(call);
            (*static_cast<NativeFn*>(userdata))(c);
            return 0;
        } catch (const std::exception& e) {
            pb_call_set_error(call, e.what());
        } catch (...) {
            pb_call_set_error(call, "host native threw a non-std exception");
        }
        return 1;
    }
    static void free_native(void* userdata) noexcept { delete static_cast<NativeFn*>(userdata); }

    pb_vm* vm_;
};

}  // namespace petal
