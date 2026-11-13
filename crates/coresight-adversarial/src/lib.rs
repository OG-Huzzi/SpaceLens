//! CoreSight adversarial input suite.
//!
//! This crate contains **no product code**. It exists so hostile, degenerate
//! and self-contradictory inputs aimed at the planning and IPC layers live in
//! one place, written as tests rather than as scattered unit cases.
//!
//! The premise: every other crate in this workspace claims its output is
//! honest under bad input. This crate tries to break that claim.

#![forbid(unsafe_code)]
