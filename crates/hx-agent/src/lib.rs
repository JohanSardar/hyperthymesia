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

mod gap;

#[cfg(test)]
mod statechart_regressions;

pub mod startup;

mod first_run;
pub mod pair;

#[cfg(all(feature = "e2e-harness", not(debug_assertions)))]
compile_error!("e2e-harness must never be enabled in release builds");

#[cfg(feature = "e2e-harness")]
pub mod harness;
