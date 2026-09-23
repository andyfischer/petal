/*
 * petal_bridge.h — C ABI for embedding the Petal VM (and petal-ui) in a host.
 *
 * The implementation is the Rust static library built from
 * integrations/petal-c-bridge in the Petal repository. Nothing
 * Rust-shaped crosses this boundary: every object is an opaque handle, every
 * value the script produces is decoded into plain C structs, and every entry
 * point catches Rust panics (reported as PB_ERR_PANIC) instead of unwinding
 * into the host.
 *
 * Model (see docs/embedding-c.md in the Petal repository for the full guide):
 *
 *   pb_vm_create                         one Env + one program + one stack
 *   pb_vm_register_native / _emitter     host natives (before the load that uses them)
 *   pb_vm_load_file / _load_source       compile; errors -> pb_vm_last_error
 *   per frame:
 *     pb_vm_input_*                      feed events as they arrive
 *     pb_vm_begin_frame                  promote input edges; bind dt/frame/time
 *     pb_vm_set_*                        bind host data (bindings)
 *     pb_vm_run                          reset_stack + run the whole program
 *     pb_vm_drain / pb_vm_drain_draw     read what the script emitted
 *   pb_vm_sources_changed + pb_vm_reload hot reload, keeping `state`
 *   pb_scenario_* + pb_vm_apply_scenario replay recorded input (headless tests)
 *
 * Threading: a pb_vm is single-threaded; use it from one thread at a time.
 *
 * Memory: strings passed in are copied. Every pointer the bridge hands out
 * (value views, draw commands, error info, string lists) is owned by the
 * bridge and documented with its lifetime; the common rule is "valid until the
 * next pb_vm_run / pb_vm_call / pb_vm_load_* / pb_vm_reload* / pb_vm_clear_views
 * on the same VM".
 */
#ifndef PETAL_BRIDGE_H
#define PETAL_BRIDGE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ─── Status codes ─────────────────────────────────────────────────────── */

typedef enum pb_status {
    PB_OK = 0,
    PB_ERR_COMPILE = 1,      /* lex/parse/module/compile error (structured items available) */
    PB_ERR_RUNTIME = 2,      /* the script failed while running */
    PB_ERR_IO = 3,           /* a file could not be read */
    PB_ERR_NOT_LOADED = 4,   /* no program is loaded */
    PB_ERR_INVALID_ARG = 5,  /* bad argument (NULL, bad UTF-8, non-canonical key, unbalanced builder…) */
    PB_ERR_NOT_FOUND = 6,    /* no such function / state variable / package */
    PB_ERR_PANIC = 7,        /* an internal Rust panic was caught; the VM may be inconsistent */
    PB_ERR_REENTRANT = 8,    /* called from inside a native callback while the VM is running */
    PB_ERR_LIMIT = 9         /* a fixed capacity was exhausted (reserved; nothing reports it today) */
} pb_status;

/* Human-readable name of a status code ("ok", "compile error", ...). Static. */
const char* pb_status_name(pb_status status);

/* Bridge version string (static), e.g. "petal-bridge 0.2.0 (ui 1, prelude 7, query 2)".
 * The ", query N" part is absent when built without the `query` cargo feature. */
const char* pb_version(void);

/* ─── Opaque handles ───────────────────────────────────────────────────── */

/* Layout check: pass sizeof(pb_value), sizeof(pb_draw_cmd), sizeof(pb_error).
 * Returns false if this header and the linked library disagree. */
bool pb_abi_check(size_t value_size, size_t draw_cmd_size, size_t error_size);

typedef struct pb_vm pb_vm;           /* one Petal Env + loaded program + stack  */
typedef struct pb_builder pb_builder; /* builds host values to hand to a script */
typedef struct pb_call pb_call;       /* one in-flight host-native invocation    */

/* ─── Errors ───────────────────────────────────────────────────────────── */

/* One diagnostic. `file` is the path/name of the source file (the entry file's
 * path when the error is in it); `line`/`column` are 1-based, 0 = unknown. */
typedef struct pb_error_item {
    const char* message;
    const char* file;
    uint32_t line;
    uint32_t column;
} pb_error_item;

/* The last failure on a VM. `message` is the full human-readable text; the
 * first item's position is mirrored into file/line/column for convenience.
 * `phase` is "lex", "parse", "module", "compile", "lower", "runtime" or "". */
typedef struct pb_error {
    pb_status code;
    const char* message;
    const char* phase;
    const char* file;
    uint32_t line;
    uint32_t column;
    const pb_error_item* items;
    size_t item_count;
} pb_error;

/* The error from the most recent failing call on `vm`, or NULL if the most
 * recent call succeeded. Owned by the VM; valid until the next call on it. */
const pb_error* pb_vm_last_error(const pb_vm* vm);

/* ─── Decoded values (script → host) ──────────────────────────────────── */

typedef enum pb_kind {
    PB_NIL = 0,
    PB_BOOL = 1,     /* integer/number = 0 or 1 */
    PB_INT = 2,      /* integer; number mirrors it as a double */
    PB_FLOAT = 3,    /* number; integer mirrors it truncated */
    PB_VEC2 = 4,     /* number = x, y = y */
    PB_STRING = 5,   /* str/str_len (UTF-8, NUL-terminated) */
    PB_LIST = 6,     /* items[count]  (f64 arrays decode as lists of PB_FLOAT) */
    PB_MAP = 7,      /* items[count], each with key/key_len; str = class name or NULL */
    PB_ENUM = 8,     /* str = variant tag, items[count] = payload */
    PB_SYMBOL = 9,   /* str = symbol name */
    PB_HANDLE = 10,  /* integer = slot, count = serial, str = handle class name */
    PB_PENDING = 11, /* an unresolved resource */
    PB_OTHER = 12    /* closure, native fn, element…; str = type name */
} pb_kind;

/* A decoded, immutable view of one Petal value. Children are contiguous. */
typedef struct pb_value {
    uint32_t kind;                 /* pb_kind */
    uint32_t count;                /* child count (LIST/MAP/ENUM) */
    const struct pb_value* items;  /* children, or NULL */
    const char* str;               /* see pb_kind; NULL when unused */
    size_t str_len;
    const char* key;               /* field name when this value is a MAP entry, else NULL */
    size_t key_len;
    int64_t integer;
    double number;
    double y;
} pb_value;

/* A list of root values (e.g. everything drained from one output buffer). */
typedef struct pb_values {
    const pb_value* items;
    size_t count;
} pb_values;

/* Inline helpers over pb_value. All accept NULL and treat it as nil. */
static inline bool pb_value_is_nil(const pb_value* v) { return !v || v->kind == PB_NIL; }
static inline bool pb_value_is_number(const pb_value* v) {
    return v && (v->kind == PB_INT || v->kind == PB_FLOAT || v->kind == PB_BOOL);
}
/* Numeric value (INT/FLOAT/BOOL), or `fallback`. */
static inline double pb_value_num(const pb_value* v, double fallback) {
    return pb_value_is_number(v) ? v->number : fallback;
}
/* Field of a MAP by name, or NULL. Linear scan (records are small). */
static inline const pb_value* pb_value_get(const pb_value* map, const char* key) {
    if (!map || map->kind != PB_MAP || !key) return NULL;
    size_t n = strlen(key);
    for (uint32_t i = 0; i < map->count; ++i) {
        const pb_value* f = &map->items[i];
        if (f->key_len == n && memcmp(f->key, key, n) == 0) return f;
    }
    return NULL;
}
/* Child `i` of a LIST/MAP/ENUM, or NULL. */
static inline const pb_value* pb_value_at(const pb_value* v, size_t i) {
    return (v && v->items && i < v->count) ? &v->items[i] : NULL;
}

/* ─── Value builder (host → script) ───────────────────────────────────── */
/*
 * A builder records a sequence of root values; containers are opened and
 * closed around their children. Inside a map, call pb_builder_key before each
 * field value. Builders are independent of any VM and reusable (pb_builder_clear).
 * Misuse (key outside a map, unbalanced end) is remembered and reported as
 * PB_ERR_INVALID_ARG by whichever call consumes the builder.
 */
pb_builder* pb_builder_new(void);
void pb_builder_free(pb_builder* b);
void pb_builder_clear(pb_builder* b);
size_t pb_builder_root_count(const pb_builder* b);

void pb_builder_nil(pb_builder* b);
void pb_builder_bool(pb_builder* b, bool v);
void pb_builder_int(pb_builder* b, int64_t v);
void pb_builder_float(pb_builder* b, double v);
void pb_builder_vec2(pb_builder* b, double x, double y);
/* A record {x, y, z} of floats. */
void pb_builder_vec3(pb_builder* b, double x, double y, double z);
void pb_builder_string(pb_builder* b, const char* utf8, size_t len);
void pb_builder_symbol(pb_builder* b, const char* name);
/* A list of `n` floats. */
void pb_builder_floats(pb_builder* b, const double* values, size_t n);
void pb_builder_begin_list(pb_builder* b);
void pb_builder_end_list(pb_builder* b);
/* A record (Petal map with string keys, insertion-ordered). */
void pb_builder_begin_map(pb_builder* b);
void pb_builder_key(pb_builder* b, const char* key);
void pb_builder_end_map(pb_builder* b);
/* An enum variant `tag(payload...)`; the payload values go between begin/end. */
void pb_builder_begin_enum(pb_builder* b, const char* tag);
void pb_builder_end_enum(pb_builder* b);

/* ─── VM lifecycle and modules ────────────────────────────────────────── */

/* A new VM with the petal-ui natives registered and the `ui` prelude as an
 * implicit import. Never returns NULL except on allocation failure/panic.
 * Script print() output is captured (read it with pb_vm_take_output) and not
 * echoed to stdout; see pb_vm_set_echo. */
pb_vm* pb_vm_create(void);
/* Destroys the VM, calling every registered native's userdata free function. */
void pb_vm_destroy(pb_vm* vm);

/* Also echo script print() lines to the process's stdout. Default: off. */
pb_status pb_vm_set_echo(pb_vm* vm, bool on);
/* Append a directory to the module search path (and discover packages in it). */
pb_status pb_vm_add_module_path(pb_vm* vm, const char* dir);
/* Register an in-memory module: `import name` resolves to `source`. */
pb_status pb_vm_register_module(pb_vm* vm, const char* name, const char* source);
/* Register a package directory (containing petal.toml), e.g. ".../petal-libs/bloom".
 * On success, `*out_name` (optional) receives the package name, valid until the next call. */
pb_status pb_vm_add_package(pb_vm* vm, const char* root, const char** out_name);
/* Add a module to the implicit-import list (initially just "ui"). Affects later loads. */
pb_status pb_vm_add_implicit_import(pb_vm* vm, const char* module_name);

/* ─── Host natives ────────────────────────────────────────────────────── */

/* Effect flags for a native (a Petal `NativeEffects` row). Combine with `|`.
 *   PB_FX_PURE      reads nothing, emits nothing, no effect (default: 0)
 *   PB_FX_PROBE     result is a pure function of args + its reads; re-evaluable
 *   PB_FX_EMITS     pushes into an output buffer
 *   PB_FX_EFFECT    does something no replay reproduces (mutates host state…)
 *   PB_FX_READS_*   input classes it reads. PB_FX_READS_HOST_DATA also makes the
 *                   bridge call note_host_read() on every call.
 *   PB_FX_PENDING_* how a Pending argument is treated (default: strict/absorb). */
enum {
    PB_FX_PURE = 0,
    PB_FX_PROBE = 1u << 0,
    PB_FX_EMITS = 1u << 1,
    PB_FX_EFFECT = 1u << 2,
    PB_FX_PENDING_EFFECTFUL = 1u << 3, /* a Pending arg makes the call a no-op */
    PB_FX_PENDING_ALLOW = 1u << 4,     /* the native sees Pending args itself */
    PB_FX_READS_POINTER = 1u << 8,
    PB_FX_READS_KEYBOARD = 1u << 9,
    PB_FX_READS_CLOCK = 1u << 10,
    PB_FX_READS_VIEWPORT = 1u << 11,
    PB_FX_READS_HOST_DATA = 1u << 12,
    PB_FX_READS_RESOURCES = 1u << 13,
    PB_FX_READS_RNG = 1u << 14,
    PB_FX_READS_BINDINGS = 1u << 15
};

/* A host native. Read arguments with pb_call_arg*, build at most one result
 * value into pb_call_result(call) (none = nil), return 0. To fail the script
 * call, call pb_call_set_error and return nonzero. The callback runs
 * synchronously inside pb_vm_run/pb_vm_call; it must not call pb_vm_* on the
 * same VM (those return PB_ERR_REENTRANT) and must not unwind (C++: catch). */
typedef int (*pb_native_fn)(pb_call* call, void* userdata);
typedef void (*pb_free_fn)(void* userdata);

/* Register `name` as a native backed by `fn`. The VM takes ownership of
 * `userdata`: `free_userdata` (optional) runs once, on pb_vm_destroy — or
 * immediately if the registration fails (bad argument), except for
 * PB_ERR_REENTRANT, where nothing was taken. Natives are compiled into
 * programs by name, so register before the pb_vm_load_* that uses them (a
 * later reload also sees them). Re-registering a name shadows the older
 * native for later loads. There is no limit on the number of natives. */
pb_status pb_vm_register_native(pb_vm* vm, const char* name, pb_native_fn fn, void* userdata,
                                pb_free_fn free_userdata, uint32_t effects);

/* Register an emitter: a native `name(args...)` that returns nil and pushes
 * `tag(args...)` (a PB_ENUM, payload = the call's arguments) into the output
 * buffer `buffer`. `tag` NULL = `name`. Effects are PB_FX_EMITS with the
 * Pending-no-op policy. This is the fast path for command streams: no host
 * code runs during the script; drain `buffer` after the run. */
pb_status pb_vm_register_emitter(pb_vm* vm, const char* name, const char* buffer, const char* tag);

/* Inside a callback: the native's registered name. */
const char* pb_call_name(const pb_call* call);
size_t pb_call_arg_count(const pb_call* call);
/* Argument `i` (0-based), or NULL when out of range. Valid during the callback. */
const pb_value* pb_call_arg(const pb_call* call, size_t i);
/* All arguments as one contiguous array (count = pb_call_arg_count). */
const pb_value* pb_call_args(const pb_call* call);
/* The builder for the result; leave empty for nil. Owned by the call. */
pb_builder* pb_call_result(pb_call* call);
/* Fail the call with `message` (copied); also return nonzero from the callback. */
void pb_call_set_error(pb_call* call, const char* message);

/* ─── Loading ─────────────────────────────────────────────────────────── */

/* Compile the file at `path` (imports resolve relative to it) and make it the
 * VM's program with a fresh stack (state starts empty). On failure the
 * previously loaded program, if any, stays loaded. */
pb_status pb_vm_load_file(pb_vm* vm, const char* path);
/* As pb_vm_load_file but from memory. `name` (optional) labels errors. */
pb_status pb_vm_load_source(pb_vm* vm, const char* source, const char* name);
bool pb_vm_is_loaded(const pb_vm* vm);

/* ─── Bindings (host → script) ────────────────────────────────────────── */
/* Scripts read them with `binding(symbol("name"))`. Bindings persist until
 * overwritten or cleared; values are copied into the VM. */
pb_status pb_vm_set_float(pb_vm* vm, const char* name, double v);
pb_status pb_vm_set_int(pb_vm* vm, const char* name, int64_t v);
pb_status pb_vm_set_bool(pb_vm* vm, const char* name, bool v);
pb_status pb_vm_set_string(pb_vm* vm, const char* name, const char* utf8);
pb_status pb_vm_set_vec2(pb_vm* vm, const char* name, double x, double y);
/* Binds the record {x, y, z}. */
pb_status pb_vm_set_vec3(pb_vm* vm, const char* name, double x, double y, double z);
pb_status pb_vm_set_floats(pb_vm* vm, const char* name, const double* values, size_t n);
/* Binds the builder's single root value (exactly one root required). The
 * builder is not modified. */
pb_status pb_vm_set_value(pb_vm* vm, const char* name, const pb_builder* value);
pb_status pb_vm_clear_binding(pb_vm* vm, const char* name);

/* ─── petal-ui input and frame info ───────────────────────────────────── */

/* Mouse buttons use petal-ui numbering. */
enum { PB_MOUSE_LEFT = 0, PB_MOUSE_RIGHT = 1, PB_MOUSE_MIDDLE = 2 };
/* Modifier bits for pb_vm_input_modifiers (scripts read mod_shift(), mod_ctrl(), mod_alt(), mod_cmd()). */
enum { PB_MOD_SHIFT = 1, PB_MOD_CTRL = 2, PB_MOD_ALT = 4, PB_MOD_CMD = 8 };

pb_status pb_vm_input_mouse_move(pb_vm* vm, int32_t x, int32_t y);
/* Raw relative motion (mouselook while the pointer is grabbed): mouse_dx/dy(). */
pb_status pb_vm_input_mouse_motion(pb_vm* vm, int32_t dx, int32_t dy);
pb_status pb_vm_input_mouse_button(pb_vm* vm, uint8_t button, bool down);
/* Wheel in lines; fractions carry across frames. */
pb_status pb_vm_input_scroll(pb_vm* vm, double dx, double dy);
/* `key` must be a canonical petal-ui key name ("a".."z", "0".."9", "space",
 * "return", "escape", "left", "shift", "f1", ...; see pb_key_is_canonical).
 * Non-canonical names are rejected with PB_ERR_INVALID_ARG. Repeated downs
 * (OS auto-repeat) re-fire key_pressed. */
pb_status pb_vm_input_key(pb_vm* vm, const char* key, bool down);
pb_status pb_vm_input_text(pb_vm* vm, const char* utf8);
pb_status pb_vm_input_modifiers(pb_vm* vm, uint32_t bits);
bool pb_key_is_canonical(const char* key);

/* Start a frame: advance input edges by `dt` seconds (key_pressed etc. cover
 * events since the previous begin_frame), and bind dt, frame_count and the
 * absolute clock time() (monotonic seconds; not a sum of dt). */
pb_status pb_vm_begin_frame(pb_vm* vm, double dt, int64_t frame, double time_seconds);
/* Drawable size in logical pixels (screen_width()/screen_height()). Persists. */
pb_status pb_vm_set_dimensions(pb_vm* vm, int32_t width, int32_t height);

/* Text measurement published to scripts, so text_width()/text_metrics()
 * match what the host's font actually draws. All values are ratios of the
 * font size. `advance_ratio`: monospace glyph advance (e.g. 0.6). */
pb_status pb_vm_set_text_metrics(pb_vm* vm, double advance_ratio);
/* Vertical metrics: y -> baseline, baseline -> line bottom, line pitch,
 * cap height and x height (defaults 0.8, 0.2, 1.2, 0.7, 0.52). */
pb_status pb_vm_set_text_vertical_metrics(pb_vm* vm, double baseline, double descent, double line_height,
                                          double cap_height, double x_height);

/* Seed random()/random_int()/choose() once, before the first run, for
 * reproducible sessions (headless tests, replays). */
pb_status pb_vm_set_seed(pb_vm* vm, uint64_t seed);

/* ─── Running ─────────────────────────────────────────────────────────── */

/* Run one frame: release previous views, clear every output buffer, reset the
 * canvas ids, reset_stack + run the whole program. `state` persists. On
 * PB_ERR_RUNTIME the buffers hold whatever was emitted before the error. */
pb_status pb_vm_run(pb_vm* vm);

/* Call a top-level Petal function (after at least one successful run).
 * `args` (optional) supplies every root as a positional argument. On success
 * `*out_result` (optional) points at the decoded return value, valid until
 * the next run/call/clear. Also releases previous views. */
pb_status pb_vm_call(pb_vm* vm, const char* function, const pb_builder* args, const pb_value** out_result);

/* Whether the last run defined a top-level function `function` (so pb_vm_call
 * would find it). Lets a host skip optional hooks without calling them. */
bool pb_vm_has_function(pb_vm* vm, const char* function);

/* Release all views/draw lists handed out so far (runs do this implicitly). */
void pb_vm_clear_views(pb_vm* vm);

/* ─── Output buffers (script → host) ─────────────────────────────────── */

/* Drain the output buffer `buffer` (values pushed by emitters or
 * push_output(symbol(buffer), v)) and decode it. The views stay valid until
 * the next run/call/clear, so several buffers can be drained per frame. */
pb_status pb_vm_drain(pb_vm* vm, const char* buffer, pb_values* out);

/* Pointer-grab request made this frame by grab_mouse()/release_mouse():
 * 1 = grab, 0 = release, -1 = none. Drains the request channel. */
int pb_vm_take_mouse_grab(pb_vm* vm);

/* ─── petal-ui draw commands ──────────────────────────────────────────── */

typedef enum pb_draw_kind {
    PB_DRAW_IMAGE = 0,        /* text = source; x y w h; color.a; radius */
    PB_DRAW_CLEAR,            /* color (a = 255) */
    PB_DRAW_RECT,             /* x y w h; color; radius (0 = square) */
    PB_DRAW_RECT_OUTLINE,     /* x y w h; color; width; radius */
    PB_DRAW_LINE,             /* x1 y1 x2 y2; color; width */
    PB_DRAW_CIRCLE,           /* cx cy; radius (= rx = ry); color */
    PB_DRAW_TEXT,             /* text; x y (top-left); size; color; font (NULL = default); weight; italic; spacing */
    PB_DRAW_TRIANGLE,         /* x1 y1 x2 y2 x3 y3; color */
    PB_DRAW_POLY,             /* points (convex fan from points[0]); color */
    PB_DRAW_POLYGON,          /* points (simple, concave OK); color */
    PB_DRAW_FAN,              /* cx cy + points (fan from center, not closed); color */
    PB_DRAW_POLYLINE,         /* points (open path, round joins); color; width */
    PB_DRAW_ELLIPSE,          /* cx cy rx ry; color */
    PB_DRAW_ELLIPSE_OUTLINE,  /* cx cy rx ry; color; width (inside the boundary) */
    PB_DRAW_ARC,              /* cx cy; r_in r_out; a0 a1 (radians, clockwise from +x, y down); color */
    PB_DRAW_RECT_GRADIENT,    /* x y w h; radius; color -> color2 along angle (radians, clockwise from +x) */
    PB_DRAW_CIRCLE_GRADIENT,  /* cx cy; radius; color (center) -> color2 (rim) */
    PB_DRAW_SHADOW,           /* x y w h radius (casting shape); blur; spread; dx dy; color */
    PB_DRAW_CLIP,             /* x y w h radius; replaces the active clip */
    PB_DRAW_CLIP_NONE,        /* clears the clip */
    PB_DRAW_CLIP_PUSH,        /* x y w h radius; intersects with the enclosing clip */
    PB_DRAW_CLIP_POP,         /* restores the clip before the matching push */
    PB_DRAW_CREATE_CANVAS,    /* id; w h */
    PB_DRAW_SET_TARGET,       /* id (0 = framebuffer) */
    PB_DRAW_DRAW_CANVAS,      /* id; x y; color.a = opacity; w h (0 = canvas size) */
    PB_DRAW_SNAPSHOT,         /* id; x y */
    PB_DRAW_BLUR_CANVAS,      /* id; radius (std dev px) */
    PB_DRAW_HOST              /* text = tag; data[data_count] = raw args (host-registered draw natives) */
} pb_draw_kind;

typedef struct pb_rgba { uint8_t r, g, b, a; } pb_rgba;

/* One petal-ui DrawCommand as a flat struct. Only the fields listed for the
 * kind in pb_draw_kind are meaningful; the rest are zero. Coordinates are
 * logical pixels, (0,0) at the top-left; colors are sRGB 0-255. */
typedef struct pb_draw_cmd {
    uint32_t kind;                   /* pb_draw_kind */
    int32_t x, y, w, h;
    int32_t x1, y1, x2, y2, x3, y3;
    int32_t cx, cy, rx, ry;
    int32_t radius;                  /* corner radius, circle radius, or blur radius */
    int32_t width;                   /* stroke width (1 = hairline) */
    pb_rgba color;                   /* fill/stroke color; gradient stop 0 */
    pb_rgba color2;                  /* gradient stop 1 */
    float r_in, r_out, a0, a1;       /* ARC */
    float angle;                     /* RECT_GRADIENT */
    int32_t blur, spread, dx, dy;    /* SHADOW */
    uint32_t id;                     /* canvas id */
    const char* text;                /* TEXT text, IMAGE source, HOST tag (NUL-terminated) */
    size_t text_len;
    const char* font;                /* TEXT: face name or NULL for the host default */
    uint16_t size;                   /* TEXT: pixel size */
    uint16_t weight;                 /* TEXT: CSS weight (400 regular, 700 bold) */
    uint8_t italic;                  /* TEXT */
    float spacing;                   /* TEXT: letter spacing px */
    const int32_t* points;           /* POLY/POLYGON/FAN/POLYLINE: x,y pairs */
    size_t point_count;              /* number of points (pairs) */
    const pb_value* data;            /* HOST: decoded args */
    size_t data_count;
} pb_draw_cmd;

/* Drain and decode the petal-ui draw commands emitted by the last run, in
 * order. Valid until the next run/call/clear. */
pb_status pb_vm_drain_draw(pb_vm* vm, const pb_draw_cmd** out_cmds, size_t* out_count);

/* ─── Hot reload ──────────────────────────────────────────────────────── */

/* Paths of every source file of the loaded program: the entry file first
 * (when loaded from a file), then each imported module with a filesystem
 * origin. Valid until the next call to this function or a load/reload. */
pb_status pb_vm_source_files(pb_vm* vm, const char* const** out_paths, size_t* out_count);

/* True when any source file changed (modification time or length), was
 * deleted, or appeared, since the program was last (re)loaded or a reload was
 * last attempted. Cheap (stat only). */
bool pb_vm_sources_changed(pb_vm* vm);

typedef struct pb_reload_result {
    uint32_t state_preserved; /* state slots whose declaration still exists */
    uint32_t state_dropped;   /* state slots dropped */
} pb_reload_result;

/* Re-read the entry file from disk, recompile, and swap the new program in
 * with transfer_state. On PB_ERR_COMPILE the old program keeps running.
 * Only valid for programs loaded with pb_vm_load_file. */
pb_status pb_vm_reload(pb_vm* vm, pb_reload_result* out);
/* The same with new source text (the program keeps its original file origin). */
pb_status pb_vm_reload_source(pb_vm* vm, const char* source, pb_reload_result* out);

/* ─── Input scenarios (petal-ui replay) ────────────────────────────────── */
/*
 * A scenario is petal-ui's declarative input script: JSON listing input
 * events keyed by frame number, plus an optional window size and frame count:
 *
 *   { "size": [800, 600], "frames": 120,
 *     "events": [ {"at": 5, "click": [100, 200]}, {"at": 9, "key": "space"},
 *                 {"at": 12, "text": "hi"}, {"at": 20, "scroll": [0, -3]} ] }
 *
 * (Also mouse_move, mouse_down/up, key_down/up, mouse_relative, modifiers;
 * see petal-ui/src/scenario.rs.) It is the same format petal-ui-run and the
 * verify tooling use, so a repro recorded there replays here. A scenario is
 * independent of any VM. Replay loop:
 *
 *   for (frame = 0; frame < n; ++frame) {
 *       pb_vm_apply_scenario(vm, sc, frame, NULL);  // events for this frame
 *       pb_vm_begin_frame(vm, dt, frame, t);          // then promote edges
 *       pb_vm_run(vm);
 *   }
 */
typedef struct pb_scenario pb_scenario;

/* An empty scenario (no events). NULL only on allocation failure/panic. */
pb_scenario* pb_scenario_new(void);
void pb_scenario_free(pb_scenario* s);
/* Replace the contents with parsed JSON. On failure (PB_ERR_INVALID_ARG for
 * malformed JSON or a non-canonical key name, PB_ERR_IO for an unreadable
 * file) the previous contents are kept and pb_scenario_error says why. */
pb_status pb_scenario_load_json(pb_scenario* s, const char* json);
pb_status pb_scenario_load_file(pb_scenario* s, const char* path);
/* Replace the contents with a deterministic pseudo-random "monkey" scenario:
 * clicks, keys and text within a width x height window over `frames` frames. */
pb_status pb_scenario_monkey(pb_scenario* s, uint64_t seed, size_t frames, int32_t width, int32_t height);
/* Why the last load failed, or NULL if it succeeded. Valid until the next load. */
const char* pb_scenario_error(const pb_scenario* s);
/* Number of primitive events (compound "click"/"key" entries expand). */
size_t pb_scenario_event_count(const pb_scenario* s);
/* One past the frame of the last event (0 when there are none). */
size_t pb_scenario_end_frame(const pb_scenario* s);
/* The scenario's "frames" field; false (and *out untouched) if absent. */
bool pb_scenario_frames(const pb_scenario* s, size_t* out_frames);
/* The scenario's "size" field; false if absent. Apply it with pb_vm_set_dimensions. */
bool pb_scenario_size(const pb_scenario* s, int32_t* out_width, int32_t* out_height);
/* The normalized scenario as JSON. Valid until the next call to this function. */
const char* pb_scenario_to_json(pb_scenario* s);

/* Feed every event `s` schedules for `frame` into the VM's input state, as the
 * pb_vm_input_* calls would. Call before pb_vm_begin_frame for that frame.
 * `*out_applied` (optional) receives the number of events applied. */
pb_status pb_vm_apply_scenario(pb_vm* vm, const pb_scenario* s, size_t frame, size_t* out_applied);

/* ─── State and output tooling ────────────────────────────────────────── */

/* All state variables as a JSON object (debug/tooling, not the hot path).
 * Valid until the next call to this function. */
const char* pb_vm_state_json(pb_vm* vm);
/* A top-level state variable by name, decoded; PB_ERR_NOT_FOUND if absent. */
pb_status pb_vm_get_state(pb_vm* vm, const char* name, const pb_value** out);
/* Take the lines the script printed since the last take. Valid until the next
 * call to this function. */
pb_status pb_vm_take_output(pb_vm* vm, const char* const** out_lines, size_t* out_count);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* PETAL_BRIDGE_H */
