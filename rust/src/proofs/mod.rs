//! Kani proof harnesses: machine-checked proofs that core-language kernels
//! meet their contracts for *all* inputs, not just the ones a test tried.
//! Compiled only under `cargo kani` (`cfg(kani)`); see
//! docs/dev/formal-verification.md for how to run them and what each proves.

mod numeric;
mod spec_sanity;
