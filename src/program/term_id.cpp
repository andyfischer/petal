#include "program/term_id.h"
#include "program/term_ref.h"

TermRef TermId::as_ref() const {
    TermRef ref;
    ref.type = TermRefType::TermIdRef;
    ref.term_id = *this;
    return ref;
}