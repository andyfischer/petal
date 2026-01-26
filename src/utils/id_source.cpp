#include "utils/id_source.h"

IDSource::IDSource() : next_id(1) {
}

u32 IDSource::take() {
    u32 id = next_id;
    next_id += 1;
    return id;
}
