//! Infrastructure adapters for Memoria.
//!
//! Every parser and library type stays inside its adapter. Adapters implement
//! the application ports and convert wire formats to application DTOs.

pub mod config;
pub mod fs;
pub mod git;
pub mod hash;
pub mod json;
pub mod markdown;
pub mod packet;
pub mod skill;
pub mod state;

pub use fs::{AtomicFileWriter, FsProjectFiles, LockFileCoordinator, SystemClock};
pub use git::GitCli;
pub use hash::Xxh3Hasher;
pub use markdown::PulldownMarkdownCodec;
pub use packet::{FsPacketInput, JsonPacketCodec};
pub use skill::FsSkillStore;
pub use state::JsonStateStore;
