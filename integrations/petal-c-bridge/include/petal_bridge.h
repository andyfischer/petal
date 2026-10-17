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
 * Model (see docs/usage/embedding-c.md in the Petal repository for the full guide):
 *
 *   pb_vm_create                         one Env + one program + one stack
 *   pb_vm_register_native / _emitter     host natives (before the load that uses them)
 *   pb_vm_declare_native_params          optional: names for named arguments
 *   pb_vm_load_file / _load_source       compile; errors -> pb_vm_last_error
 *   per frame:
 *     pb_vm_input_*                      feed events as they arrive
 *     pb_vm_begin_frame                  promote input edges; bind dt/frame/time
 *     pb_vm_set_*                        bind host data (bindings)
 *     pb_vm_run                          reset_stack + run the whole program
 *     pb_vm_drain / pb_vm_drain_draw     read what the script emitted
 *   pb_vm_sources_changed + pb_vm_reload hot reload, keeping `state`
 *     (pb_vm_changed_sources names the edited files; a value or layout
 *     edit is taken up without recompiling)
 *   pb_vm_set_config                     set one config value now, no file write
 *   pb_vm_tests_json + pb_vm_test_*      run the script's test blocks, across frames
 *   pb_scenario_* + pb_vm_apply_scenario replay recorded input (headless tests)
 *   pb_source_*                          read and edit source text as a config file
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
    PB_ERR_LIMIT = 9,        /* a fixed capacity was exhausted (reserved; nothing reports it today) */
    PB_ERR_NEEDS_RELOAD = 10 /* pb_vm_set_config: the value cannot be set without recompiling */
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
 * `phase` is "lex", "parse", "module", "compile", "lower", "link" (a declared
 * native or a required interface this VM does not have), "runtime" or "". */
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
    PB_ENUM = 8,     /* str = variant tag ("Shape.circle"), items[count] = payload */
    PB_SYMBOL = 9,   /* str = symbol name */
    PB_HANDLE = 10,  /* integer = slot, count = serial, str = handle class name */
    PB_PENDING = 11, /* an unresolved resource */
    PB_OTHER = 12,   /* closure, native fn, element…; str = type name */
    PB_VEC3 = 13     /* number = x, y = y, z = z */
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
    double y;                      /* VEC2/VEC3 y component */
    double z;                      /* VEC3 z component */
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
/* A native vec3 (decodes back as PB_VEC3). */
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
/* An enum variant `tag(payload...)`; the payload values go between begin/end.
 * Under the tag "Enum.variant", with the payload in declaration order, it is
 * that variant of the script's enum (it equals the script's own and matches
 * its patterns). The tag is not checked against any program. */
void pb_builder_begin_enum(pb_builder* b, const char* tag);
void pb_builder_end_enum(pb_builder* b);

/* ─── VM lifecycle and modules ────────────────────────────────────────── */

/* A new VM with the petal-ui natives and the `ui` package registered. Nothing
 * is imported implicitly: a script that uses them starts with `import ui`.
 * Never returns NULL except on allocation failure/panic.
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
/* Register a package directory (containing petal.toml), e.g. ".../core-runtime/bloom".
 * On success, `*out_name` (optional) receives the package name, valid until the next call. */
pb_status pb_vm_add_package(pb_vm* vm, const char* root, const char** out_name);
/* Register a package from memory — for a host with no filesystem, or one that
 * compiles its interface in. `module_names[i]` is package-relative ("menu",
 * "ui/menu"; a ".ptl" suffix is stripped) and `sources[i]` its source; the
 * module named like the package is the one `import name` loads. This is the
 * form `petal bundle` emits. A registration here wins over a package of the
 * same name on the search path or bundled in the runtime.
 *
 * A package whose modules declare natives (`native fn`) is an *interface
 * package*: it is how a script reaches this host's natives (`import name`),
 * and registering it gives the natives registered so far the parameter names
 * it declares. Register natives first, then the package.
 *
 * A module named "petal.toml" is the package's manifest rather than a module:
 * it carries the version, [dependencies] and [requires] (the host interfaces
 * the package needs; see pb_vm_provides). */
pb_status pb_vm_register_package(pb_vm* vm, const char* name, const char* const* module_names,
                                 const char* const* sources, size_t count);
/* A host's drift check: every native the named packages declare is
 * registered in this VM, and every native registered beyond the builtins is
 * declared by one of them. Name every interface package the host answers for
 * in one call (a pb_vm_create VM answers for "ui" plus the host's own).
 * PB_OK when they agree; PB_ERR_NOT_FOUND otherwise, with the differences in the
 * error message and in `*out_report` (optional, valid until the next call;
 * "" on success). */
pb_status pb_vm_verify_interface(pb_vm* vm, const char* const* packages, size_t count,
                                 const char** out_report);
/* Announce that this VM provides the host interface `name` at `version`
 * ("0.4.0"). A package states what it needs in its petal.toml
 * (`[requires] name = ">=0.4"`); loading a program whose packages require an
 * interface the VM does not provide, or provides at a version the requirement
 * rejects, fails before any line runs. Call it where the interface's natives
 * are registered. A pb_vm_create VM already provides "ui" (petal-ui) and
 * "core". PB_ERR_INVALID_ARG when `version` is not a dotted number. */
pb_status pb_vm_provides(pb_vm* vm, const char* name, const char* version);
/* pb_vm_provides for a package registered in this VM (pb_vm_register_package
 * with its "petal.toml", or pb_vm_add_package), at the version its manifest
 * gives — so the announcement cannot drift from the package.
 * PB_ERR_NOT_FOUND when there is no such package or it has no version. */
pb_status pb_vm_provides_package(pb_vm* vm, const char* name);
/* Check-only mode: programs loaded afterwards are compiled but not linked
 * against this VM's natives, so a script written for another host can be
 * checked here. Default: off. */
pb_status pb_vm_set_check_only(pb_vm* vm, bool on);
/* Where `import ./x` resolves for a program loaded from a string
 * (pb_vm_load_source): a directory, or NULL for registered modules only. */
pb_status pb_vm_set_inline_import_dir(pb_vm* vm, const char* dir);
/* Compile as if the file at `path` held `source`: every later import that
 * resolves to that file reads this text instead of the one on disk (a
 * relative import is a file, so a module registered in memory cannot stand in
 * for it). A reload compiles the override again, so edits to the file itself
 * do not reach the program while it stands. The entry file is not an import:
 * pb_vm_load_source / pb_vm_reload_source run other text in its place. For a
 * host that rewrites a script before running it without touching the file. */
pb_status pb_vm_override_file(pb_vm* vm, const char* path, const char* source);

/* ─── Host natives ────────────────────────────────────────────────────── */

/* Effect flags for a native (a Petal `NativeEffects` row). Combine with `|`.
 *   PB_FX_PURE      reads nothing, emits nothing, no effect (default: 0)
 *   PB_FX_PROBE     result is a pure function of args + its reads; re-evaluable
 *   PB_FX_EMITS     pushes into an output buffer. A pb_native_fn callback has no
 *                   way to push, so this is right only for pb_vm_register_emitter
 *                   (which sets it itself); a callback that queues work on the
 *                   host is PB_FX_EFFECT. See docs/usage/ffi.md "Embedder pitfalls".
 *   PB_FX_EFFECT    does something no replay reproduces (mutates host state…)
 *   PB_FX_READS_*   input classes it reads. PB_FX_READS_HOST_DATA also makes the
 *                   bridge call note_host_read() on every call.
 *   PB_FX_PENDING_* how a Pending argument is treated (default: strict/absorb).
 *   PB_FX_CONST     a constant function: the same arguments give the same
 *                   result for as long as the VM lives, it reads no host state
 *                   at all (no time, input, viewport, theme, fonts, random,
 *                   game or editor state — tracked by a READS flag or not) and
 *                   does nothing but return. Petal may then call it ONCE for a
 *                   top-level table and keep the answer until the script's
 *                   source changes, so a wrong PB_FX_CONST is a stale value
 *                   forever. Opt-in: PB_FX_PURE (0) does NOT imply it, and a
 *                   native without it is always re-evaluated. Combined with
 *                   PROBE, EMITS, EFFECT or any READS flag it is ignored.
 *                   When in doubt, leave it off. See docs/usage/ffi.md
 *                   "Constant natives". */
enum {
    PB_FX_PURE = 0,
    PB_FX_PROBE = 1u << 0,
    PB_FX_EMITS = 1u << 1,
    PB_FX_EFFECT = 1u << 2,
    PB_FX_PENDING_EFFECTFUL = 1u << 3, /* a Pending arg makes the call a no-op */
    PB_FX_PENDING_ALLOW = 1u << 4,     /* the native sees Pending args itself */
    PB_FX_CONST = 1u << 5,             /* constant function of its arguments */
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

/* Declare the parameter names of a registered native (a callback or an
 * emitter), so scripts may call it with named arguments:
 *
 *   pb_vm_declare_native_params(vm, "spawn", "kind, x, y, speed?");
 *   // script: spawn(kind: "spark", x: 10, y: 20)
 *
 * `params` lists the names in the order the native reads its arguments,
 * comma-separated; a trailing `?` marks an optional parameter, and optional
 * ones come last. Named arguments are put in that order before the native
 * runs, so the callback still reads them by index. A native that takes
 * several argument counts may declare each form with its own call; a named
 * call uses the first form it fits. Without a declaration a named argument is
 * a script error ("builtin 'spawn' does not accept named arguments").
 * Declare before the pb_vm_load_* that uses the names. PB_ERR_INVALID_ARG for
 * an unknown native or a malformed list. */
pb_status pb_vm_declare_native_params(pb_vm* vm, const char* name, const char* params);

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
/* Where the script call is written: the file as diagnostics name it, and the
 * 1-based line and column of the call expression. Each out-pointer is
 * optional. False (nothing written) when the call has no source position (a
 * call the host made itself). `*out_file` is valid during the callback. */
bool pb_call_location(pb_call* call, const char** out_file, uint32_t* out_line, uint32_t* out_column);
/* Record an expectation of the host's own (an `expect_pixel`, say), as the
 * script's expect() does: `ok` true counts a check that held; false records a
 * failure at the position of the script call and the script carries on. The
 * failure reaches the test's result when a test is running and
 * pb_vm_take_expect_failures_json otherwise. `check` names what was checked
 * (NULL = the native's name); `message` is the whole failure in one line.
 * Do not also fail the call: an expectation is not an error. */
void pb_call_expect(pb_call* call, bool ok, const char* check, const char* message);
/* Ask the test this call runs in to stop once the callback returns, so the
 * host can run frames and then carry on with pb_vm_test_resume: how a
 * `wait_frames(n)` native is written (see "Tests" below). The call still
 * returns its result normally. Returns 0, or nonzero when nothing can resume
 * the call: outside a test begun with pb_vm_test_begin (an ordinary run, a
 * pb_vm_call), or inside a callback of map/filter/reduce/forEach/sort. The
 * call's error is then already set to the reason and the script call fails
 * when the callback returns; return from it without doing the work.
 * pb_call_can_suspend asks without failing anything. */
int pb_call_suspend(pb_call* call);
bool pb_call_can_suspend(pb_call* call);

/* ─── Loading ─────────────────────────────────────────────────────────── */

/* Compile the file at `path` (imports resolve relative to it) and make it the
 * VM's program with a fresh stack (state starts empty). On failure the
 * previously loaded program, if any, stays loaded, and the VM watches for the
 * fix (pb_vm_sources_changed); pb_vm_reload then retries this load. */
pb_status pb_vm_load_file(pb_vm* vm, const char* path);
/* As pb_vm_load_file but from memory. `name` (optional) labels errors. */
pb_status pb_vm_load_source(pb_vm* vm, const char* source, const char* name);
bool pb_vm_is_loaded(const pb_vm* vm);

/* ─── Bindings (host → script) ────────────────────────────────────────── */
/* Scripts read them with `binding(symbol("name"))`, after `import io`.
 * Bindings persist until overwritten or cleared; values are copied into the VM. */
pb_status pb_vm_set_float(pb_vm* vm, const char* name, double v);
pb_status pb_vm_set_int(pb_vm* vm, const char* name, int64_t v);
pb_status pb_vm_set_bool(pb_vm* vm, const char* name, bool v);
pb_status pb_vm_set_string(pb_vm* vm, const char* name, const char* utf8);
pb_status pb_vm_set_vec2(pb_vm* vm, const char* name, double x, double y);
/* Binds a native vec3. */
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
/* Per-glyph advances for proportional fonts: `advances[codepoint]` is that
 * glyph's advance as a ratio of the font size (one table serves every size).
 * Codepoints past the end use the pb_vm_set_text_metrics ratio; NULL/0 binds
 * an empty table (uniform advances). The values are copied. */
pb_status pb_vm_set_text_advances(pb_vm* vm, const double* advances, size_t n);
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

/* Start the loaded program over with empty `state`, without recompiling (a
 * fresh stack on the same program). PB_ERR_NOT_LOADED without a program. */
pb_status pb_vm_restart(pb_vm* vm);

/* Call a top-level Petal function (after at least one successful run).
 * `args` (optional) supplies every root as a positional argument. On success
 * `*out_result` (optional) points at the decoded return value, valid until
 * the next run/call/clear. Also releases previous views. */
pb_status pb_vm_call(pb_vm* vm, const char* function, const pb_builder* args, const pb_value** out_result);

/* pb_vm_call with the arguments as JSON text, for a host that receives them
 * that way (a tool call from an agent, a command from a socket).
 *
 *   args_json   a JSON array: positional arguments;
 *               a JSON object: arguments by parameter name, in any order,
 *               where a parameter with a default may be left out;
 *               NULL, "" or `null`: no arguments.
 *
 * Values are spelled as in a state document (pb_vm_save_state): plain JSON
 * is itself, with a whole number an int and an object a record, and
 * {"$petal": ...} objects spell what JSON cannot (an enum variant, a class
 * instance, a vec2/vec3, a symbol, inf/nan).
 *
 * JSON does not tell 3 from 3.0, so the function's declared parameter types
 * decide (the same data fn_info() returns): for `x: float` a whole number
 * arrives as a float, and for `n: int` a float with no fraction arrives as an
 * int. An argument of the wrong kind for a parameter declared int, float,
 * num, string, bool, list or record is PB_ERR_INVALID_ARG naming the
 * parameter, and the function is not run. `null` always passes, as nil; a
 * parameter with another type or none takes what it is sent; nothing inside
 * a list or record is looked at. An overloaded function is resolved by
 * argument count and its arguments pass as sent.
 *
 * PB_ERR_NOT_FOUND when there is no such function; PB_ERR_RUNTIME when the
 * arguments do not bind (an unknown name, a required parameter missing) or
 * the function fails. The result is a view, as from pb_vm_call. */
pb_status pb_vm_call_json(pb_vm* vm, const char* function, const char* args_json,
                          const pb_value** out_result);
/* The declared parameters of a top-level function, as JSON:
 *   {"name": "toll", "params": [{"name": "cents", "type": "int",
 *                               "has_default": true, "default_source": "25"}]}
 * "type" is the annotation as written and "default_source" the default's
 * source text; each is null where the declaration has none. "params" is null
 * for an overloaded function. NULL (PB_ERR_NOT_FOUND) when the last run
 * defined no such function. Valid until the next call to this function. */
const char* pb_vm_function_json(pb_vm* vm, const char* function);

/* Whether the last run defined a top-level function `function` (so pb_vm_call
 * would find it). Lets a host skip optional hooks without calling them. */
bool pb_vm_has_function(pb_vm* vm, const char* function);

/* Release all views/draw lists handed out so far (runs do this implicitly). */
void pb_vm_clear_views(pb_vm* vm);

/* ─── Tests ───────────────────────────────────────────────────────────── */
/*
 * A script's `test "title" do ... end` blocks (docs/testing.md), run by the
 * host with its natives present. A test is a function nothing calls; its
 * expect() / expect_eq() / expect_near() calls record failures and carry on.
 *
 *   pb_vm_run(vm);                                  // functions exist now
 *   // for each entry of pb_vm_tests_json(vm):
 *   pb_vm_test_begin(vm, "__test_0");
 *   for (;;) {
 *       int step = pb_vm_test_resume(vm, 0);
 *       if (step == PB_TEST_SUSPENDED) { run_the_frames_it_asked_for(); continue; }
 *       break;                                      // PB_TEST_FINISHED, or -1
 *   }
 *   const char* result = pb_vm_test_result_json(vm);
 *
 * A test spans frames by suspending: a host native (wait_frames(n)) notes n
 * and calls pb_call_suspend; pb_vm_test_resume then returns
 * PB_TEST_SUSPENDED with the test set aside, the host runs frames as it
 * always does (pb_vm_begin_frame, pb_vm_run, drain), and the next
 * pb_vm_test_resume continues on the line after the call. When it resumes,
 * a `var` or `state var` the test reads with `get` is live; a `let` or plain
 * `state` it captured is the value from when the test began.
 *
 * The test sees the program as the last run left it and its state changes
 * stay: for isolation, pb_vm_restart + pb_vm_run before each test, or
 * pb_vm_save_state once and pb_vm_load_state + pb_vm_run before each.
 * One test at a time per VM. A reload while a test waits ends it with an
 * error on the next resume.
 */

/* The loaded program's test blocks, the entry file's and every module's, in
 * declaration order:
 *   [{"name": "the door opens", "function": "__test_0", "module": null,
 *     "file": "game.ptl", "path": "/abs/game.ptl", "line": 12}]
 * "function" is what pb_vm_test_begin takes; "module" is null for the entry
 * file (how a runner leaves out a library's tests); "path" is null for a
 * source with no file. NULL with no program loaded. Valid until the next call
 * to this function. */
const char* pb_vm_tests_json(pb_vm* vm);

typedef enum pb_test_step {
    PB_TEST_FINISHED = 0,     /* the body returned or raised: read pb_vm_test_result_json */
    PB_TEST_SUSPENDED = 1,    /* a native called pb_call_suspend: run frames, resume again */
    PB_TEST_OUT_OF_STEPS = 2  /* max_steps instructions ran: resume again, or abort */
} pb_test_step;

/* Start a test. `function` is a "function" of pb_vm_tests_json (any
 * top-level function of no parameters works). None of it runs yet.
 * PB_ERR_NOT_FOUND when the last run defined no such function;
 * PB_ERR_RUNTIME when a test is already in progress or the program has not
 * completed a run. */
pb_status pb_vm_test_begin(pb_vm* vm, const char* function);
/* Run the test in progress until it finishes, is suspended, or about
 * `max_steps` instructions have run in this call (0 = no bound; a host with
 * a time limit resumes in slices and looks at the clock). Returns a
 * pb_test_step, or -1 (PB_ERR_INVALID_ARG in pb_vm_last_error) when no test
 * is in progress. Call it between runs, never from a native. Releases
 * previous views. */
int pb_vm_test_resume(pb_vm* vm, uint64_t max_steps);
/* Give up on the test in progress: its result is what it recorded so far,
 * failed, with `why` (NULL = "aborted by the host") as its error.
 * PB_ERR_INVALID_ARG when no test is in progress. */
pb_status pb_vm_test_abort(pb_vm* vm, const char* why);
/* The "function" of the test in progress, or NULL when there is none. Valid
 * until the next call to this function. */
const char* pb_vm_test_in_progress(pb_vm* vm);
/* The result of the test that last finished or was aborted:
 *   {"function": "__test_0", "name": "the door opens", "passed": false,
 *    "checks": 3, "failed": 1, "error": null,
 *    "failures": [{"check": "expect_eq", "file": "game.ptl", "line": 14,
 *                  "column": 3, "message": "expected 3, got 4",
 *                  "actual": "4", "expected": "3"}]}
 * "name" is the title (null when the function is not a test block); "error"
 * the runtime error that ended the test, if one did; a failure may also
 * carry "tolerance" and "note" (the message argument). "failures" holds at
 * most 1000; "failed" counts all. "" before the first. Valid until the next
 * test finishes; does not touch pb_vm_last_error. */
const char* pb_vm_test_result_json(const pb_vm* vm);
/* Begin and resume in one call, for a test with nothing to wait for (one
 * that suspends is aborted, with an error saying it needs frames). Returns
 * the result (as pb_vm_test_result_json), or NULL with no program loaded. A
 * failing test is not a failing call: read "passed". */
const char* pb_vm_run_test(pb_vm* vm, const char* function);
/* The expectations that failed OUTSIDE tests, in the program's ordinary runs
 * and in functions the host called, since the last take:
 *   {"checked": 120, "failed": 2, "failures": [...]}
 * with the failures as in pb_vm_test_result_json. "checked" and "failed"
 * count since the program was loaded or restarted, not since the last take.
 * Between takes the log holds 1000 failures and only counts the rest. A
 * test's failures never land here. NULL with no program loaded. Valid until
 * the next call to this function. */
const char* pb_vm_take_expect_failures_json(pb_vm* vm);

/* ─── Output buffers (script → host) ─────────────────────────────────── */

/* Drain the output buffer `buffer` (values pushed by emitters or
 * push_output(symbol(buffer), v), which a script reaches with `import io`)
 * and decode it. The views stay valid until
 * the next run/call/clear, so several buffers can be drained per frame. */
pb_status pb_vm_drain(pb_vm* vm, const char* buffer, pb_values* out);

/* Pointer-grab request made this frame by grab_mouse()/release_mouse():
 * 1 = grab, 0 = release, -1 = none. Drains the request channel. */
int pb_vm_take_mouse_grab(pb_vm* vm);

/* ─── petal-ui draw commands ──────────────────────────────────────────── */

typedef enum pb_draw_kind {
    PB_DRAW_IMAGE = 0,        /* text = source; x y w h (destination); color.a = opacity; radius;
                               * color.rgb = tint, multiplied into the image (255,255,255 = none);
                               * x1 y1 x2 y2 = source rect x, y, w, h in image pixels (x2 = 0: whole image);
                               * x3 y3 cx cy = nine-slice insets left, top, right, bottom in image pixels (all 0: none) */
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
    PB_DRAW_HOST,             /* text = tag; data[data_count] = raw args (host-registered draw natives) */
    /* The four below are sent only to a host that asked for them with
     * pb_vm_set_draw_caps; by default the bridge lowers them (a batch to one
     * RECT per entry, a retained layer to its content moved and clipped). */
    PB_DRAW_RECTS,            /* points = point_count rects, five int32 each: x, y, w, h, then the colour as
                               * (r << 24) | (g << 16) | (b << 8) | a. Draw in order. Needs PB_DRAW_CAP_BATCH_RECTS */
    PB_DRAW_CACHE_BEGIN,      /* id; w h. Start recording retained layer `id`: the commands up to the matching
                               * CACHE_END are its content, in the layer's own coordinates ((0,0) = its top-left),
                               * and are not drawn where they stand. Recordings nest. Needs PB_DRAW_CAP_RETAINED_LAYERS */
    PB_DRAW_CACHE_END,        /* id. End of the recording; replaces whatever `id` held */
    PB_DRAW_CACHE_DRAW        /* id; x y. Draw retained layer `id` with its top-left at (x, y), clipped to its size.
                               * The content is from the most recent recording, on this frame or an earlier one */
} pb_draw_kind;

/* What the host draws itself instead of having the bridge lower it. */
#define PB_DRAW_CAP_BATCH_RECTS      1u  /* send PB_DRAW_RECTS */
#define PB_DRAW_CAP_RETAINED_LAYERS  2u  /* send PB_DRAW_CACHE_BEGIN / _END / _DRAW */

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
    const int32_t* points;           /* POLY/POLYGON/FAN/POLYLINE: x,y pairs. RECTS: x,y,w,h,rgba fives */
    size_t point_count;              /* number of points (pairs), or of rects (fives) */
    const pb_value* data;            /* HOST: decoded args */
    size_t data_count;
} pb_draw_cmd;

/* Drain and decode the petal-ui draw commands emitted by the last run, in
 * order. Valid until the next run/call/clear. */
pb_status pb_vm_drain_draw(pb_vm* vm, const pb_draw_cmd** out_cmds, size_t* out_count);

/* Declare which commands this host draws itself (PB_DRAW_CAP_* bits). Call
 * once after creating the vm; the default, 0, has the bridge lower them.
 *
 * With PB_DRAW_CAP_RETAINED_LAYERS the host keeps each layer's content (a
 * texture, typically) from CACHE_BEGIN/_END and draws it on CACHE_DRAW. It
 * may free a layer that a frame's commands neither recorded nor drew. If it
 * loses a layer it is still being asked to draw (a device reset, a CACHE_DRAW
 * for an id it does not have), it calls pb_vm_invalidate_layers and every
 * layer is recorded again on the next run. */
pb_status pb_vm_set_draw_caps(pb_vm* vm, uint32_t caps);
pb_status pb_vm_invalidate_layers(pb_vm* vm);

/* ─── petal-ui targets ────────────────────────────────────────────────── */

/* One rect a script registered on its last frame: by interact(r, id), by a
 * prelude widget (a button is registered under its label unless given an
 * id), or by ui_target(id, r). The rect is in logical pixels and already
 * clipped to what was visible, so its centre is a point a click lands on. */
typedef struct pb_ui_target {
    const char* id;      /* NUL-terminated; "" when the rect was given no id */
    double x, y, w, h;
    int64_t layer;
    uint8_t blocks;      /* 1 = takes the pointer from lower layers */
    uint8_t modal;       /* 1 = not a rect: its layer is modal (w = h = 0) */
} pb_ui_target;

/* The rects the most recent run registered, in the order registered. Valid
 * until the next run/call/clear. */
pb_status pb_vm_ui_targets(pb_vm* vm, const pb_ui_target** out_targets, size_t* out_count);

/* Look one up by id (the last registered under it). Returns 1 and fills
 * `out` if found, 0 if not, -1 on a bad argument. `out->id` is valid until
 * the next run/call/clear. */
int pb_vm_ui_target(pb_vm* vm, const char* id, pb_ui_target* out);

/* ─── Hot reload ──────────────────────────────────────────────────────── */

/* Paths of every source file of the loaded program: the entry file first
 * (when loaded from a file), then each imported module with a filesystem
 * origin. Valid until the next call to this function or a load/reload. */
pb_status pb_vm_source_files(pb_vm* vm, const char* const** out_paths, size_t* out_count);

/* True when any source file changed (modification time or length), was
 * deleted, or appeared, since the program was last (re)loaded or a reload was
 * last attempted. Cheap (stat only). While broken — after a failed
 * pb_vm_load_file, or a failed reload of a file-loaded program — the watched
 * set also covers every .ptl file under the entry file's directory (hidden
 * directories skipped), and a new .ptl file there counts as a change, until a
 * load or reload succeeds: the fix may land in a module the broken edit
 * imports. */
bool pb_vm_sources_changed(pb_vm* vm);

/* The source files pb_vm_sources_changed reports as changed, in
 * pb_vm_source_files order (while broken: the entry file first, then other
 * watched files, then new ones). 0 when nothing changed or nothing is
 * watched. Valid until the next call to this function. */
pb_status pb_vm_changed_sources(pb_vm* vm, const char* const** out_paths, size_t* out_count);

typedef struct pb_reload_result {
    uint32_t state_preserved; /* state slots whose declaration still exists */
    uint32_t state_dropped;   /* state slots dropped */
} pb_reload_result;

/* Re-read the entry file from disk and bring the running program up to date
 * with it and with its module files, doing only the work the edit calls for
 * (docs/features/hot-reload.md):
 *
 *   - nothing changed, or only whitespace / comments / layout: source
 *     positions move; nothing is recompiled and nothing is dropped;
 *   - only literal values changed (0.35 became 0.4, "red" became "blue",
 *     #ff2e88 became #ff2e80), each keeping its type: the values are written
 *     into the running program; nothing is recompiled;
 *   - anything else: recompile, and swap the new program in with
 *     transfer_state.
 *
 * Whichever it is, the program and its state afterwards are what the full
 * recompile would have left. pb_vm_last_reload_kind says which path was taken.
 * On PB_ERR_COMPILE the old program keeps running. Only valid for programs
 * loaded with pb_vm_load_file. After a failed pb_vm_load_file it retries that
 * load instead (fresh state; `out` counts nothing preserved or dropped). */
pb_status pb_vm_reload(pb_vm* vm, pb_reload_result* out);
/* The same with new source text for the entry file (the program keeps its
 * original file origin; module files are still read from where they live). */
pb_status pb_vm_reload_source(pb_vm* vm, const char* source, pb_reload_result* out);

typedef enum pb_reload_kind {
    PB_RELOAD_UNCHANGED = 0,  /* no source text changed */
    PB_RELOAD_RELOCATED = 1,  /* layout only: source positions moved */
    PB_RELOAD_PATCHED = 2,    /* literal values written in place */
    PB_RELOAD_RECOMPILED = 3  /* recompiled, state transferred */
} pb_reload_kind;

/* How the last successful pb_vm_reload / pb_vm_reload_source / pb_vm_set_config
 * was carried out (a pb_reload_kind), or -1 before the first one. */
int pb_vm_last_reload_kind(pb_vm* vm);

/* The same in detail, as JSON; NULL before the first one:
 *
 *   {"outcome": "patched",          // unchanged | relocated | patched | recompiled
 *    "change": "values",            // none | values | constructs | full
 *    "files": ["config.ptl"],       // the source files whose text changed
 *    "state_preserved": 12, "state_dropped": 0,
 *    "values": [                    // change == "values": each literal that changed
 *      {"path": "POST.bloom",       //   binding path, or null outside a top-level let
 *       "config": true,             //   inside a `config let`
 *       "old": 0.2, "new": 0.4, "line": 3}],
 *    "constructs": [                // change == "constructs": what differs
 *      {"kind": "fn",               //   fn | let | config let | var | state | enum |
 *       "name": "draw",             //   class | import | statement (name null)
 *       "edit": "changed",          //   added | removed | changed
 *       "line": 40}],
 *    "reason": "...",               // change == "full": why
 *    "fallback": "...",             // the edit looked incremental and was
 *                                   // recompiled anyway: why
 *    "statics": {                   // absent when the program does not run with
 *                                   // the static-value cache, or has not run
 *      "kept": 41,                  //   static values the edit could not have
 *                                   //   changed: still held
 *      "rebuilt": 3,                //   static values the next run builds
 *      "rebuilt_names": ["TILES"],  //   the declarations among them
 *      "deferred": ["GRID"],        //   changed by a pb_vm_set_config, slow to
 *                                   //   build: rebuilt when the drag pauses
 *      "stale_seeds": [             //   state that kept its value although its
 *        {"name": "grid",           //   initializer read something that changed
 *         "read": "TILES"}],        //   (null: its own declaration changed)
 *      "stale_caches": [            //   state filled behind a test (a cache)
 *        {"name": "paths",          //   from something the edit changed: it
 *         "read": "ROADS"}],        //   keeps what it built
 *      "cleared": null},            //   why everything was rebuilt, if it was
 *    "new_diagnostics": [           // what the edit introduced (the items of
 *      {"severity": "warning",      // pb_vm_diagnostics_json); empty unless
 *       "message": "...",           // the reload recompiled
 *       "file": "game.ptl", "line": 3, "column": 5}]}
 *
 * Valid until the next call to this function. */
const char* pb_vm_last_reload_json(pb_vm* vm);

/* Set one value of a top-level binding in the running program now, without
 * the file changing on disk: the live half of a drag.
 *
 *   pb_vm_set_config(vm, NULL, "POST.effects[2].amount", "0.35");
 *
 * `path` is a binding path as for pb_source_set_value and `value_json` a value
 * in the same JSON (a bare number is a float; an integer is {"int": 3}, so a
 * drag over an integer literal sends that form). `file` is the path of the
 * source file that holds the
 * binding; NULL finds it (exactly one source file must bind the name at its
 * top level).
 *
 * The edit is the one pb_source_set_value makes, applied to the text the
 * running program holds for that file, and taken up without recompiling. The
 * program afterwards is exactly what reloading that edited file would give.
 * pb_vm_source_text returns the edited text: write it to the file when the
 * drag ends, and the reload that follows has nothing to do. To abandon the
 * drag instead, pb_vm_reload: the file's own values come back.
 *
 * PB_ERR_NEEDS_RELOAD when the value has a different type or shape from the
 * one written (10 to 10.5, a list that grew, a field that is not there):
 * write the file and reload for those. PB_ERR_NOT_FOUND when the path or file
 * names nothing, PB_ERR_INVALID_ARG for a malformed path or JSON, or a name
 * bound in more than one file. A failed call changes nothing.
 *
 * Like a reload, a successful call ends the lifetime of views handed out by
 * the last run, and the functions of the program are callable again only
 * after the next pb_vm_run. */
pb_status pb_vm_set_config(pb_vm* vm, const char* file, const char* path,
                           const char* value_json);

/* The text the running program holds for one of its source files: what it was
 * compiled from, plus every pb_vm_set_config since. `file` is the file's path;
 * NULL is the entry file. NULL when it is not a source file of the program.
 * Valid until the next call to this function. */
const char* pb_vm_source_text(pb_vm* vm, const char* file);

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
 * see core-libs/petal-ui/src/scenario.rs.) It is the same format petal-ui-run and the
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
 * `*out_applied` (optional) receives the number of events applied.
 *
 * A {"at": N, "click_target": "id"} entry is resolved here, against the rects
 * the VM's last run registered (pb_vm_ui_target): a move to the target's
 * centre and a left press at N, the release at N+1. A target that is not
 * registered applies nothing; check pb_vm_ui_target first to tell. */
pb_status pb_vm_apply_scenario(pb_vm* vm, const pb_scenario* s, size_t frame, size_t* out_applied);

/* ─── Source text: reading and editing without running ─────────────────── */
/*
 * A pb_source is Petal source text plus the tools to treat it as a
 * configuration file (docs/usage/config-files.md, docs/features/source-preservation.md):
 * read every top-level binding's static value, and change values where they
 * are written, keeping every other character — comments, layout, the author's
 * spelling of the numbers next to it. It is independent of any VM; the host
 * saves the edited text, and a running program picks it up by hot reload.
 *
 *   pb_source* s = pb_source_new(file_text);
 *   const char* json = pb_source_bindings_json(s);             // what the file sets
 *   pb_source_set_value(s, "POST.effects[2].amount", "0.35");  // one number changes
 *   pb_source_append(s, "POST.effects", "{\"rec\":{\"effect\":\"crt\"}}");
 *   pb_source_remove(s, "POST.effects[0]");
 *   write_file(path, pb_source_text(s));
 *   pb_source_free(s);
 *
 * A path is a top-level name followed by `.field` (into a record literal) and
 * `[index]` (into a list literal, or the arguments of a call such as
 * `vec3(0.3, -1.0, 0.5)`) steps. The name alone is the binding's whole
 * right-hand side; the last binding of a name is the one read and edited.
 */
typedef struct pb_source pb_source;

/* A source holding a copy of `text` (NULL = empty). NULL only on allocation
 * failure/panic. Text that does not parse is accepted here and reported by
 * the calls below. */
pb_source* pb_source_new(const char* text);
void pb_source_free(pb_source* s);
/* The current text. Valid until the next successful edit of `s` or the free. */
const char* pb_source_text(const pb_source* s);

/* Every top-level binding as a JSON array, in source order:
 *
 *   [{"name": "POST", "config": true, "exported": true, "line": 77,
 *     "text": "{exposure: 1.45}", "comment": "The night look.",
 *     "value": {"rec": {"exposure": 1.45}}},
 *    {"name": "atmosphere", ..., "reason": "a function declaration"}]
 *
 * `config`: written `config let`. `text`: the right-hand side as written.
 * `comment`: the comment block directly above. A binding has either `value`
 * (its static value: literals, lists, records, calls, variants of the file's
 * enums and names bound above) or `reason` (why it has none: arithmetic, a
 * function, state, an enum's own declaration, ...). Values:
 * floats are numbers, strings strings, lists arrays, nil null, and
 *   {"int": 3}                        an integer
 *   {"rec": {"key": value, ...}}      a record, keys in source order
 *   {"call": "vec3", "args": [...]}   a call, unevaluated
 *   {"variant": "Mode.dark"}          a variant of an enum the file declares;
 *                                     with a payload, "args": [...] as well
 *   {"color": "#ff2e88"}              a color literal, as #rrggbb or #rrggbbaa
 *                                     in lowercase (pb_source_expr gives the
 *                                     spelling in the file)
 * NULL when the text does not parse (pb_source_error says why). Valid until
 * the next call to this function on `s`. */
const char* pb_source_bindings_json(pb_source* s);

/* The text of the expression at `path` as written ("#ff2e88", "0.020000"),
 * or NULL (pb_source_error). Valid until the next call to this function. */
const char* pb_source_expr(pb_source* s, const char* path);

/* Make the expression at `path` read `value` (Petal source for exactly one
 * expression). Only that expression's text changes; a value already spelled
 * that way changes nothing. PB_ERR_NOT_FOUND when the path names no literal
 * in the source, PB_ERR_INVALID_ARG for a malformed path or a value that is
 * not one expression (or would leave the text unparseable), PB_ERR_COMPILE
 * when the text does not parse. A failed call leaves the text as it was. */
pb_status pb_source_set(pb_source* s, const char* path, const char* value);

/* The value edits. Each takes a value as JSON — the format
 * pb_source_bindings_json writes, so a value read can be changed and handed
 * straight back — and edits the literal in place:
 *
 *   - what already reads as the new value is not touched: unchanged elements
 *     and fields keep their text, comments and alignment, and writing a value
 *     that already holds changes nothing at all;
 *   - a changed number or color keeps the old token's habits (`3.50` becomes
 *     `4.00`, `#F80` becomes `#0F8`); whether a number is an int or a float is
 *     the value's say ({"int": 2} against 2.0);
 *   - new elements and fields are formatted like their siblings (one line or
 *     several, padding, separators, trailing comma, indentation), or like the
 *     rest of the file when the container is empty.
 *
 * All four return PB_ERR_NOT_FOUND when the path names nothing,
 * PB_ERR_INVALID_ARG for a malformed path or JSON or an edit that makes no
 * sense there (an index past the end, a field of a number), and
 * PB_ERR_COMPILE when the text does not parse. A failed call leaves the text
 * as it was; the text is never left unparseable. */

/* Make the value at `path` read as `value_json`. A list or record whose shape
 * changed is edited element by element (inserts, removes, moves), not
 * rewritten. A record field that does not exist yet is added to its record. */
pb_status pb_source_set_value(pb_source* s, const char* path, const char* value_json);
/* Insert a value so that `path` names it afterwards: "effects[1]" inserts
 * before the current element 1 of a list (or call), "POST.bloom" adds a field
 * to a record — before the sibling field `before`, or last when `before` is
 * NULL (or names no field). A field that already exists is set instead. */
pb_status pb_source_insert(pb_source* s, const char* path, const char* value_json,
                           const char* before);
/* Append a value to the list (or call arguments) at `path`. */
pb_status pb_source_append(pb_source* s, const char* path, const char* value_json);
/* Remove the list element or record field at `path`. An element on its own
 * line takes the line with it; comments on other lines stay. Removing a field
 * that is not there succeeds and changes nothing. */
pb_status pb_source_remove(pb_source* s, const char* path);

/* Why the last pb_source_bindings_json / _expr / _set / value edit on `s`
 * failed, or NULL if it succeeded. Valid until the next such call. */
const char* pb_source_error(const pb_source* s);

/* ─── State and output tooling ────────────────────────────────────────── */

/* All state variables as a JSON object (debug/tooling, not the hot path):
 * keyed by display name and lossy in the values, so it is for reading, not
 * for putting back; pb_vm_save_state is the form that loads.
 * Valid until the next call to this function. */
const char* pb_vm_state_json(pb_vm* vm);
/* A top-level state variable by name, decoded; PB_ERR_NOT_FOUND if absent. */
pb_status pb_vm_get_state(pb_vm* vm, const char* name, const pb_value** out);
/* ── State snapshots ──
 * The whole `state` of the program as one JSON document, and back: a save
 * point, an undo step, a fork into another VM, a session that outlives the
 * process. The format is the one `petal run --state-storage` writes
 * ("format": "petal-state", versioned; docs/CLI.md "State between runs").
 *
 * pb_vm_save_state returns the document (NULL with no program loaded; valid
 * until the next call to it). It holds every slot: top-level and module
 * state, and the slots a declaration inside a function, a loop or a
 * state(key) group owns. Call it between runs.
 *
 * pb_vm_load_state REPLACES the state with a document's: a slot the document
 * does not hold starts from its initial value on the next run. The VM must
 * have a program loaded; it may be the VM that saved, at any later point, or
 * another VM (or process) that loaded the same program. Slots are matched by
 * the ids a hot reload uses, so a document saved before a code edit loads
 * after it by the same rules a reload keeps state: state whose declaration
 * was renamed or removed is not loaded, and newly declared state starts from
 * its initializer. PB_ERR_INVALID_ARG, with nothing changed, when the text is
 * not a state document, is a version this build does not read, or holds
 * slots of which none belongs to the loaded program.
 *
 * What a document does not carry:
 *   - a value that is or holds a function, a host handle, a pending result
 *     or a UI element: the whole slot is left out of the save, and after a
 *     load that state starts from its initial value;
 *   - whether a value was frozen (freeze()): it comes back equal and
 *     unfrozen;
 *   - anything that is not `state`: host bindings, output buffers, input,
 *     the petal-ui registries, and whatever the host's own natives keep.
 * Everything else comes back as it was, with its type: ints and floats stay
 * apart, records keep their field order, enum variants, class instances,
 * vec2/vec3, f64 arrays, symbols and inf/nan are themselves.
 *
 * The next pb_vm_run sees the loaded state. A function called before that
 * run (pb_vm_call) still reads the top-level state it captured at the last
 * run, so run a frame after a load before calling into the script.
 *
 * pb_vm_state_report_json says what the last save or load did, and is where
 * a host learns what was left out:
 *   {"op": "save", "slots": 12, "skipped": [{"name": "on_done", "holds": "a function"}]}
 *   {"op": "load", "slots": 11, "dropped": ["old_name"]}
 * "" before the first; valid until the next save or load. It does not touch
 * pb_vm_last_error. */
const char* pb_vm_save_state(pb_vm* vm);
pb_status pb_vm_load_state(pb_vm* vm, const char* document);
/* The same through a file; PB_ERR_IO when it cannot be written or read. */
pb_status pb_vm_save_state_file(pb_vm* vm, const char* path);
pb_status pb_vm_load_state_file(pb_vm* vm, const char* path);
const char* pb_vm_state_report_json(const pb_vm* vm);

/* Call memoization (on by default, or as the PETAL_POLICY environment
 * variable says): a user-function call whose arguments and recorded reads are
 * what they were last run is replayed instead of run. It pays off for UI-like
 * programs whose inputs mostly repeat; a game whose inputs change every frame
 * runs faster with it off. Output is the same either way. Takes effect at the
 * next run. */
pb_status pb_vm_set_memo(pb_vm* vm, bool on);
bool pb_vm_memo(pb_vm* vm);
/* The static-value cache (on by default, or as PETAL_POLICY says): a
 * top-level value that depends on no dynamic data (a table of literals, one
 * built from other such tables by functions that read nothing else, a
 * `const`) is built by the first run after a load or a reload and kept for
 * the runs after it. Output is the same either way. Setting it takes effect
 * at the next run, which lowers the program again. pb_vm_static_cache says
 * whether runs keep values right now. */
pb_status pb_vm_set_static_cache(pb_vm* vm, bool on);
bool pb_vm_static_cache(pb_vm* vm);
/* Drop every kept static value; the next run builds them again. A reload
 * drops only the ones the edit reaches. For a host that changed something a
 * native it registered PB_FX_CONST answers from, which the cache cannot
 * see. */
pb_status pb_vm_clear_static_values(pb_vm* vm);
/* Whether a reload keeps the static values the edit cannot have changed
 * (true, the default) or rebuilds every one. "statics" in
 * pb_vm_last_reload_json says what a reload kept. */
pb_status pb_vm_set_static_follow(pb_vm* vm, bool on);
/* A slow rebuild under a drag. While pb_vm_set_config is called
 * continuously, a static value whose last build took at least threshold_ms
 * keeps its old value for every tick after the first, and is rebuilt by the
 * first run that starts quiet_ms after the last tick, by the next reload,
 * or by pb_vm_flush_static_rebuilds. The value
 * being dragged never waits. Defaults: 4 and 150. A negative threshold_ms
 * rebuilds on every tick. */
pb_status pb_vm_set_static_debounce(pb_vm* vm, double threshold_ms, double quiet_ms);
/* The drag ended: rebuild what it put off on the next run. Returns how many
 * values were waiting, -1 with no program loaded. */
int pb_vm_flush_static_rebuilds(pb_vm* vm);
/* What the cache holds, as a JSON object: "enabled", "slots", "filled",
 * "runs", "skips", "skipped_instructions", "clears" (counters since the
 * program was loaded), "rejected" (every value that could not be kept, as
 * {"name", "at", "const", "holds"}; such a value is built on every run) and
 * "warnings" (the rejections in words; only those no earlier call handed
 * out since the last load or reload, so log them after a frame and each is
 * said once). Valid until the next call to this function. */
const char* pb_vm_static_values_json(pb_vm* vm);
/* What the compile of the loaded program found and did not stop for, as
 * `petal check` prints it ("" = nothing; NULL with no program loaded).
 * `errors_only` leaves the warnings out. Print it after a load and after a
 * reload: a `const` that is not constant is such an error, the program runs
 * with that binding built on every run like a `let`, and because the check
 * goes by the natives THIS vm registered PB_FX_CONST, no tool but the host
 * can report it. Valid until the next call to this function. */
const char* pb_vm_diagnostics_text(pb_vm* vm, bool errors_only);
/* The same findings as data, for a host with a diagnostics surface of its
 * own:
 *   {"errors": 1, "warnings": 2,
 *    "items": [{"severity": "warning", "message": "...", "file": "game.ptl",
 *               "line": 3, "column": 5}]}
 * "file" is null for a program loaded from an unnamed string. NULL with no
 * program loaded. Valid until the next call to this function. */
const char* pb_vm_diagnostics_json(pb_vm* vm);
/* What the last reload introduced: the findings of its compile, warnings and
 * errors, that the program before the edit did not have, as `petal check`
 * prints them. This is what to print after a reload: a large program's
 * standing warnings are not repeated on every save, and the one the edit
 * just wrote is. "" when the edit introduced nothing, when the reload did
 * not recompile (a layout or value edit is not checked again) and before the
 * first reload; NULL with no program loaded. pb_vm_last_reload_json carries
 * the same list as "new_diagnostics". Valid until the next call to this
 * function. */
const char* pb_vm_new_diagnostics_text(pb_vm* vm, bool errors_only);
/* The VM profiler (off by default): instructions per opcode and per function,
 * native calls and the time spent in them, collections. Turning it on clears
 * earlier counts; while on, every run pays a per-instruction hook (roughly 2x
 * slower), so profile a representative stretch rather than a whole session.
 * It may be turned on before pb_vm_load_*, and the report then covers the
 * first run (frame 0: boot). */
pb_status pb_vm_set_profiling(pb_vm* vm, bool on);
/* The time from which a native call counts as slow, for the report's "native
 * calls of N us or longer, by calling line" section (default 1). Kept across
 * pb_vm_set_profiling. PB_ERR_INVALID_ARG for a negative or non-finite value. */
pb_status pb_vm_set_profile_slow_native_us(pb_vm* vm, double microseconds);
/* The profile report (text, top `top_n` rows per section) for the loaded
 * program. NULL if nothing is loaded. Valid until the next call to this function. */
const char* pb_vm_profile_report(pb_vm* vm, size_t top_n);
/* Take the lines the script printed since the last take. Valid until the next
 * call to this function. */
pb_status pb_vm_take_output(pb_vm* vm, const char* const** out_lines, size_t* out_count);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* PETAL_BRIDGE_H */
