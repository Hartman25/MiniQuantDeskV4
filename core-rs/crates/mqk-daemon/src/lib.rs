//! mqk-daemon library target.
//!
//! Exposes the router and state for integration tests.
//! The binary `main.rs` depends on this library target.

pub mod api_types;
pub mod routes;
pub mod state;
