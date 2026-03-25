//! mqk-daemon library target.
//!
//! Exposes the router and state for integration tests.
//! The binary `main.rs` depends on this library target.

pub mod api_types;
pub mod bind;
pub mod cors;
pub mod decision;
pub mod dev_gate;
pub mod notify;
pub mod routes;
pub mod state;
pub mod suppression;
