//! Thin native capture: policy, durable delivery, and source observations only.
pub mod config;
pub mod envelope;
pub mod native_messaging;
pub mod outbox;
pub mod policy;
mod runtime;
pub mod upload;
pub use runtime::{RunOptions, run, status};

mod media;

mod accessibility;

mod input;
