#pragma once

#include "standard_headers.h"
#include <functional>

struct TermRef;

struct TermId {
    BlockId block_id;
    TermLocalId term_local_id;

    TermId(BlockId block_id, TermLocalId term_local_id) : block_id(block_id), term_local_id(term_local_id) {}

    static TermId None() {
        return TermId(0, 0);
    }

    bool operator==(const TermId& other) const {
        return block_id == other.block_id && term_local_id == other.term_local_id;
    }

    TermRef as_ref() const;
};

namespace std {
    template<>
    struct hash<TermId> {
        size_t operator()(const TermId& term_id) const {
            return hash<u64>{}((static_cast<u64>(term_id.block_id) << 32) | term_id.term_local_id);
        }
    };
}
