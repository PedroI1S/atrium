//! Policy engine rev 4 implementation modules (contract §13).
//!
//! **Not wired into `SessionGate` until P5.** `dev`'s evaluator stays live
//! while P1–P3 implement their modules in parallel behind this frozen
//! function-level interface.
//!
//! Ownership (contract §13):
//! - `lex.rs`, `units.rs`: P1 (§3, §4)
//! - `paths.rs`: P2 (§5)
//! - `tools.rs`, `rules.rs`: P3 (§6, §7)
//! - `evaluate.rs`: P5 (§1, §2)

#![allow(dead_code, clippy::todo)]

pub(crate) mod evaluate;
pub(crate) mod lex;
pub(crate) mod paths;
pub(crate) mod rules;
pub(crate) mod tools;
pub(crate) mod units;
