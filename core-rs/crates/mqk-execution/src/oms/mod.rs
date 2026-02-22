//! OMS (Order Management System) — Patch L4
//!
//! Provides an explicit, deterministic state machine for live order lifecycle.
//! All state transitions are type-checked at compile time; illegal transitions
//! return a `TransitionError` that callers MUST treat as a halt signal.

pub mod state_machine;
