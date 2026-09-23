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
    REQUIRE(pb_vm_load_source(vm, "note(\"hi\", {n: 2})\nprint(\"from c\")", "smoke.ptl") == PB_OK);
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

    /* A compile error keeps the VM usable and reports a position. */
    REQUIRE(pb_vm_load_source(vm, "let = 3", "bad.ptl") == PB_ERR_COMPILE);
    const pb_error* err = pb_vm_last_error(vm);
    REQUIRE(err != NULL && err->code == PB_ERR_COMPILE && err->line == 1);
    REQUIRE(pb_vm_run(vm) == PB_OK);
    REQUIRE(pb_vm_last_error(vm) == NULL);

    pb_vm_destroy(vm);
    printf("c_smoke: ok\n");
    return 0;
}
