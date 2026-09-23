//! Shared FFI plumbing: status codes, structured errors, C-string helpers and
//! the panic firewall every `extern "C"` entry point goes through.

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Mirrors `pb_status` in `petal_bridge.h`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Status {
    Ok = 0,
    Compile = 1,
    Runtime = 2,
    Io = 3,
    NotLoaded = 4,
    InvalidArg = 5,
    NotFound = 6,
    Panic = 7,
    Reentrant = 8,
    Limit = 9,
}

impl Status {
    pub fn name(self) -> &'static CStr {
        match self {
            Status::Ok => c"ok",
            Status::Compile => c"compile error",
            Status::Runtime => c"runtime error",
            Status::Io => c"i/o error",
            Status::NotLoaded => c"no program loaded",
            Status::InvalidArg => c"invalid argument",
            Status::NotFound => c"not found",
            Status::Panic => c"internal panic",
            Status::Reentrant => c"reentrant call",
            Status::Limit => c"limit exceeded",
        }
    }
}

/// One diagnostic of a [`BridgeError`] (Rust-side form).
#[derive(Clone, Debug)]
pub struct ErrorItem {
    pub message: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// A failure inside the bridge, before it is published as a `pb_error`.
#[derive(Clone, Debug)]
pub struct BridgeError {
    pub code: Status,
    pub message: String,
    pub phase: String,
    pub items: Vec<ErrorItem>,
}

impl BridgeError {
    pub fn new(code: Status, message: impl Into<String>) -> Self {
        BridgeError {
            code,
            message: message.into(),
            phase: String::new(),
            items: Vec::new(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(Status::InvalidArg, message)
    }

    /// A structured front-end error. `entry_name` labels items that belong to
    /// the entry file (petal reports those with no file name).
    pub fn from_load(err: &petal::error::LoadError, entry_name: &str) -> Self {
        let items: Vec<ErrorItem> = err
            .items
            .iter()
            .map(|item| ErrorItem {
                message: item.message.clone(),
                file: item.file.clone().unwrap_or_else(|| entry_name.to_string()),
                line: item.span.map(|s| s.start.line).unwrap_or(0),
                column: item.span.map(|s| s.start.column).unwrap_or(0),
            })
            .collect();
        BridgeError {
            code: Status::Compile,
            message: err.to_string(),
            phase: err.phase.as_str().to_string(),
            items,
        }
    }

    /// A runtime error message from the VM. Petal ends the first line with the
    /// failing position: `" [line N, column M]"` in the entry file, or
    /// `" [module.ptl line N, column M]"` inside an imported module. The
    /// position (and file) is recovered from either form.
    pub fn runtime(message: String, entry_name: &str) -> Self {
        // Runtime errors may be multi-line (snippet, stack trace); the
        // position is on the first line, which is the error itself.
        let first = message.lines().next().unwrap_or("");
        let item = match parse_position_suffix(first) {
            Some((text, file, line, column)) => ErrorItem {
                message: text.to_string(),
                file: file.unwrap_or(entry_name).to_string(),
                line,
                column,
            },
            None => ErrorItem {
                message: message.clone(),
                file: String::new(),
                line: 0,
                column: 0,
            },
        };
        BridgeError {
            code: Status::Runtime,
            message,
            phase: "runtime".into(),
            items: vec![item],
        }
    }
}

/// Split `"message [file line N, column M]"` (file optional) into its parts.
/// The bracket must end the line.
fn parse_position_suffix(line: &str) -> Option<(&str, Option<&str>, u32, u32)> {
    let body = line.strip_suffix(']')?;
    let open = body.rfind(" [")?;
    let inner = &body[open + 2..];
    // `inner` is "line N, column M" or "<file> line N, column M".
    let at = if inner.starts_with("line ") {
        0
    } else {
        inner.rfind(" line ")? + 1
    };
    let (row, col) = inner[at + "line ".len()..].split_once(", column ")?;
    let row: u32 = row.trim().parse().ok()?;
    let col: u32 = col.trim().parse().ok()?;
    let file = if at == 0 {
        None
    } else {
        Some(inner[..at - 1].trim())
    };
    Some((&line[..open], file.filter(|f| !f.is_empty()), row, col))
}

#[cfg(test)]
mod tests {
    use super::parse_position_suffix;

    #[test]
    fn positions_with_and_without_a_file() {
        assert_eq!(
            parse_position_suffix("Division by zero [line 4, column 9]"),
            Some(("Division by zero", None, 4, 9))
        );
        assert_eq!(
            parse_position_suffix("bad [x] thing [props.ptl line 3, column 12]"),
            Some(("bad [x] thing", Some("props.ptl"), 3, 12))
        );
        assert_eq!(
            parse_position_suffix("in dir/my mod.ptl [dir/my mod.ptl line 1, column 2]"),
            Some(("in dir/my mod.ptl", Some("dir/my mod.ptl"), 1, 2))
        );
        assert_eq!(parse_position_suffix("no position here"), None);
        assert_eq!(parse_position_suffix("odd [line x, column 2]"), None);
    }
}

pub type BResult<T> = Result<T, BridgeError>;

/// Mirrors `pb_error_item`.
#[repr(C)]
pub struct PbErrorItem {
    pub message: *const c_char,
    pub file: *const c_char,
    pub line: u32,
    pub column: u32,
}

/// Mirrors `pb_error`.
#[repr(C)]
pub struct PbError {
    pub code: Status,
    pub message: *const c_char,
    pub phase: *const c_char,
    pub file: *const c_char,
    pub line: u32,
    pub column: u32,
    pub items: *const PbErrorItem,
    pub item_count: usize,
}

/// A [`BridgeError`] published in C form. Owns every string the `PbError`
/// points at; boxed by the VM so the pointers stay put.
pub struct PublishedError {
    _strings: Vec<CString>,
    _items: Vec<PbErrorItem>,
    pub c: PbError,
}

impl PublishedError {
    pub fn new(err: BridgeError) -> Box<PublishedError> {
        let mut strings = Vec::new();
        let mut keep = |s: &str| -> *const c_char {
            let c = cstring_lossy(s);
            let p = c.as_ptr();
            strings.push(c);
            p
        };
        let message = keep(&err.message);
        let phase = keep(&err.phase);
        let items: Vec<PbErrorItem> = err
            .items
            .iter()
            .map(|i| PbErrorItem {
                message: keep(&i.message),
                file: keep(&i.file),
                line: i.line,
                column: i.column,
            })
            .collect();
        let (file, line, column) = match items.first() {
            Some(first) => (first.file, first.line, first.column),
            None => (keep(""), 0, 0),
        };
        let c = PbError {
            code: err.code,
            message,
            phase,
            file,
            line,
            column,
            items: items.as_ptr(),
            item_count: items.len(),
        };
        Box::new(PublishedError {
            _strings: strings,
            _items: items,
            c,
        })
    }
}

/// A `CString` from arbitrary Rust text; interior NULs (never expected in
/// Petal strings, but possible) are replaced rather than failing.
pub fn cstring_lossy(s: &str) -> CString {
    CString::new(s).unwrap_or_else(|_| CString::new(s.replace('\0', "\u{FFFD}")).unwrap())
}

/// Borrow a C string argument as UTF-8.
///
/// # Safety
/// `p` must be NULL or point at a NUL-terminated string valid for the call.
pub unsafe fn arg_str<'a>(p: *const c_char, what: &str) -> BResult<&'a str> {
    if p.is_null() {
        return Err(BridgeError::invalid(format!("{what} is NULL")));
    }
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .map_err(|_| BridgeError::invalid(format!("{what} is not valid UTF-8")))
}

/// Text of a caught panic payload.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Run `f` behind the panic firewall, for entry points with no VM to record
/// an error on (builders, free functions). A panic yields `fallback`.
pub fn guard<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}
