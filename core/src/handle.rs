//! Handles — opaque references to host-owned foreign objects.
//!
//! A `Value::Handle` carries a (class, slot, serial) triple. The class indexes
//! into the `HandleClass` registry on `Env`; slot/serial address an object in
//! the host's own storage (typically a slot map with generation counters).
//! See docs/dev/unreal-ffi-proposal.md.

use std::fmt;

use crate::native_fn::{NativeResult, PetalCxt};

/// Index into `Env`'s handle-class registry.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct HandleClassId(pub u16);

/// The payload of a `Value::Handle`: which class it belongs to plus the
/// host-side (slot, serial) address of the underlying object.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct HandleVal {
    pub class: HandleClassId,
    pub slot: u32,
    pub serial: u32,
}

impl fmt::Display for HandleVal {
    /// `handle(class:slot#serial)` — class names live on `Env`, so the
    /// value-level rendering is numeric.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "handle({}:{}#{})", self.class.0, self.slot, self.serial)
    }
}

/// A host-registered class of foreign objects reachable through handles.
/// The callbacks take (slot, serial) and consult the host's own object table.
pub struct HandleClass {
    pub name: String,
    /// Whether (slot, serial) still refers to a live object.
    pub is_valid: Box<dyn Fn(u32, u32) -> bool>,
    /// Human-readable description of the object, for error messages.
    pub describe: Box<dyn Fn(u32, u32) -> String>,
    /// Method dispatch: called with the receiver as arg 1 (later chunk).
    pub call_method: Box<dyn Fn(&mut PetalCxt, &str) -> NativeResult>,
}
