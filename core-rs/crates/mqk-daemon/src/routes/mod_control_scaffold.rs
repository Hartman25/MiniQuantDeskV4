/// Scaffold module include snippet (manual merge):
///
/// In your existing `crates/mqk-daemon/src/routes/mod.rs` (or wherever routes are composed),
/// add:
///   pub mod control;
/// and then merge `control::router(...)` into your top-level router.
///
/// This file exists only to prevent "where do I wire it" confusion.
