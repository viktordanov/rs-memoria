//! Application layer for Memoria: use cases, inward-facing ports, and DTOs.
//!
//! Use cases receive borrowed port implementations, build one immutable
//! project snapshot through the shared pipeline, apply domain rules, and
//! return plain result values. Presentation renders those values; adapters
//! implement the ports.

pub mod config;
pub mod diff;
pub mod error;
pub mod gitignore;
pub mod guidance;
pub mod packet;
pub mod ports;
pub mod snapshot;
pub mod usecases;

pub use error::{AppError, Detail, Diagnostic, ExitClass, Severity};
pub use ports::Services;
