//! Seams between the conversion core and its environment; the core owns the traits, this module
//! re-exports them under their old paths next to the native implementations.

pub(crate) mod clock;
pub(crate) mod executor;
pub(crate) mod node_job;
pub(crate) mod point_source;
pub(crate) mod spool_store;
