//! petal-bridge: a C ABI over the Petal VM and petal-ui.
//!
//! It owns a Petal `Env` per `pb_vm`, registers the petal-ui natives and `ui`
//! prelude, and exposes everything through `include/petal_bridge.h` (plus the
//! header-only C++ wrapper `include/petal.hpp`). The guide is
//! `docs/embedding-c.md` at the repository root.
//!
//! - [`vm`]: lifecycle, loading, bindings, running, output buffers, petal-ui
//!   input/draw, hot reload, state tooling.
//! - [`natives`]: host-registered natives (C callbacks and emitters), each a
//!   boxed Petal native owning its callback and userdata — no globals.
//! - [`scenario`]: petal-ui input scenarios (JSON replay keyed by frame).
//! - [`view`]: decoding Petal values into flat `pb_value` trees.
//! - [`builder`]: building host values (`pb_builder`).
//! - [`draw`]: petal-ui `DrawCommand` → `pb_draw_cmd`.
//! - [`ffi`]: status codes, structured errors, the panic firewall.
//!
//! Rules every entry point follows: no panic crosses the boundary (each is
//! wrapped in `catch_unwind`), no Rust type leaks (handles are opaque, data is
//! `#[repr(C)]`), and every pointer handed out has a documented owner and
//! lifetime.
//!
//! The crate is also an `rlib`: a Rust crate that exposes its own C ABI can
//! reuse [`vm::Vm`], [`view::ViewArena`], [`draw::DrawList`] and
//! [`builder::HostValue`] directly, and [`vm::VmHandle::into_raw`] hands a
//! `Vm` it configured itself to C as a `pb_vm*`.

// Every `extern "C"` function here takes raw pointers from C; their safety
// contracts are the ones documented in `include/petal_bridge.h` (valid handles
// from the matching constructor, NUL-terminated strings, sized buffers), and
// NULL is checked everywhere. Repeating that per function adds nothing.
#![allow(clippy::missing_safety_doc, clippy::not_unsafe_ptr_arg_deref)]

pub mod builder;
pub mod draw;
pub mod ffi;
pub mod natives;
pub mod scenario;
pub mod view;
pub mod vm;

use std::ffi::c_char;

use ffi::Status;

#[unsafe(no_mangle)]
pub extern "C" fn pb_status_name(status: Status) -> *const c_char {
    status.name().as_ptr()
}

/// Static version string, built once.
fn version() -> &'static std::ffi::CStr {
    static VERSION: std::sync::OnceLock<std::ffi::CString> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        #[cfg(feature = "query")]
        let query = format!(", query {}", petal_query::QUERY_VERSION);
        #[cfg(not(feature = "query"))]
        let query = String::new();
        ffi::cstring_lossy(&format!(
            "petal-bridge {} (ui {}, prelude {}{query})",
            env!("CARGO_PKG_VERSION"),
            petal_ui::UI_VERSION,
            petal_ui::PRELUDE_LEVEL,
        ))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_version() -> *const c_char {
    version().as_ptr()
}

/// Layout check between the header the host compiled against and this
/// library: the host passes its `sizeof`s; false means a mismatch.
#[unsafe(no_mangle)]
pub extern "C" fn pb_abi_check(value_size: usize, draw_cmd_size: usize, error_size: usize) -> bool {
    value_size == std::mem::size_of::<view::PbValue>()
        && draw_cmd_size == std::mem::size_of::<draw::PbDrawCmd>()
        && error_size == std::mem::size_of::<ffi::PbError>()
}
