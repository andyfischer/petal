/* Plain-C smoke test: petal_bridge.h must be usable from C, not just C++. */
#include "petal_bridge.h"

#include <stdio.h>
#include <string.h>

#define REQUIRE(cond)                                                     \
    do {                                                                  \
        if (!(cond)) {                                                    \
            fprintf(stderr, "c_smoke: %s:%d: %s\n", __FILE__, __LINE__, #cond); \
            return 1;                                                     \
        }                                                                 \
    } while (0)

int main(void) {
    REQUIRE(pb_abi_check(sizeof(pb_value), sizeof(pb_draw_cmd), sizeof(pb_error)));

    pb_vm* vm = pb_vm_create();
    REQUIRE(vm != NULL);
    REQUIRE(pb_vm_register_emitter(vm, "note", "notes", NULL) == PB_OK);
    /* The host's interface package declares its natives; scripts import it.
     * Registered from memory here; pb_vm_add_package takes a directory. */
    const char* const names[] = {"host"};
    const char* const sources[] = {"pub native fn note(...args)\n"};
    REQUIRE(pb_vm_register_package(vm, "host", names, sources, 1) == PB_OK);
    const char* const answers_for[] = {"ui", "host"};
    const char* report = NULL;
    REQUIRE(pb_vm_verify_interface(vm, answers_for, 2, &report) == PB_OK);
    REQUIRE(report != NULL && report[0] == '\0');
    REQUIRE(pb_vm_load_source(vm, "import host\nnote(\"hi\", {n: 2})\nprint(\"from c\")", "smoke.ptl") == PB_OK);
    REQUIRE(pb_vm_run(vm) == PB_OK);

    pb_values notes;
    REQUIRE(pb_vm_drain(vm, "notes", &notes) == PB_OK);
    REQUIRE(notes.count == 1);
    const pb_value* note = &notes.items[0];
    REQUIRE(note->kind == PB_ENUM && strcmp(note->str, "note") == 0);
    REQUIRE(note->count == 2);
    REQUIRE(strcmp(pb_value_at(note, 0)->str, "hi") == 0);
    REQUIRE(pb_value_num(pb_value_get(pb_value_at(note, 1), "n"), 0) == 2.0);

    const char* const* lines;
    size_t n;
    REQUIRE(pb_vm_take_output(vm, &lines, &n) == PB_OK);
    REQUIRE(n == 1 && strcmp(lines[0], "from c") == 0);

    /* A program loaded from memory has no files to change. */
    const char* const* changed = &lines[0];
    REQUIRE(pb_vm_changed_sources(vm, &changed, &n) == PB_OK);
    REQUIRE(n == 0);

    /* Native vec3s cross the boundary as PB_VEC3 in both directions. */
    REQUIRE(pb_vm_set_vec3(vm, "v", 1.0, 2.0, 3.0) == PB_OK);
    REQUIRE(pb_vm_load_source(vm, "import io; import host\nnote(binding(symbol(\"v\")) * 2)", "vec3.ptl") == PB_OK);
    REQUIRE(pb_vm_run(vm) == PB_OK);
    REQUIRE(pb_vm_drain(vm, "notes", &notes) == PB_OK);
    REQUIRE(notes.count == 1);
    const pb_value* v = pb_value_at(&notes.items[0], 0);
    REQUIRE(v->kind == PB_VEC3 && v->number == 2.0 && v->y == 4.0 && v->z == 6.0);

    /* A compile error keeps the VM usable and reports a position. */
    REQUIRE(pb_vm_load_source(vm, "let = 3", "bad.ptl") == PB_ERR_COMPILE);
    const pb_error* err = pb_vm_last_error(vm);
    REQUIRE(err != NULL && err->code == PB_ERR_COMPILE && err->line == 1);
    REQUIRE(pb_vm_run(vm) == PB_OK);
    REQUIRE(pb_vm_last_error(vm) == NULL);

    /* A JSON call by parameter name, and a state document put back. */
    REQUIRE(pb_vm_load_source(vm,
                              "state n = 0\nn += 1\n"
                              "fn scale(x: float, by: int = 2)\n  x * by\nend\n",
                              "state.ptl") == PB_OK);
    REQUIRE(pb_vm_run(vm) == PB_OK);
    const pb_value* scaled = NULL;
    REQUIRE(pb_vm_call_json(vm, "scale", "{\"x\": 3}", &scaled) == PB_OK);
    REQUIRE(scaled->kind == PB_FLOAT && scaled->number == 6.0);
    REQUIRE(pb_vm_call_json(vm, "scale", "{\"x\": \"wide\"}", &scaled) == PB_ERR_INVALID_ARG);
    REQUIRE(strstr(pb_vm_function_json(vm, "scale"), "\"type\":\"float\"") != NULL);
    const char* saved_view = pb_vm_save_state(vm);
    REQUIRE(saved_view != NULL);
    char saved[512];
    REQUIRE(strlen(saved_view) < sizeof saved);
    strcpy(saved, saved_view);
    REQUIRE(pb_vm_run(vm) == PB_OK && pb_vm_run(vm) == PB_OK);
    const pb_value* n_now = NULL;
    REQUIRE(pb_vm_get_state(vm, "n", &n_now) == PB_OK && n_now->integer == 3);
    REQUIRE(pb_vm_load_state(vm, saved) == PB_OK);
    REQUIRE(strstr(pb_vm_state_report_json(vm), "\"op\":\"load\"") != NULL);
    REQUIRE(pb_vm_get_state(vm, "n", &n_now) == PB_OK && n_now->integer == 1);
    REQUIRE(pb_vm_load_state(vm, "{}") == PB_ERR_INVALID_ARG);
    REQUIRE(strstr(pb_vm_diagnostics_json(vm), "\"items\":[]") != NULL);

    pb_vm_destroy(vm);
    printf("c_smoke: ok\n");
    return 0;
}
