//! Infrastructure adapters for Memoria.
//!
//! Every parser and library type stays inside its adapter. Adapters implement
//! the application ports and convert wire formats to application DTOs.

pub mod agent_locations;
pub mod bounded_process;
pub mod client_probe;
pub mod config;
pub mod fs;
pub mod git;
pub mod github_workflow;
pub mod hash;
pub mod hook_config;
pub mod hooks;
pub mod json;
pub mod lock_codec;
pub mod markdown;
pub mod packet;
pub mod repository_ignore;
pub mod skill;
pub mod state;

pub use agent_locations::EnvAgentLocations;
pub use bounded_process::SelfStatusProcess;
pub use client_probe::{ClientProbe, CommandClientProbe};
pub use fs::{AtomicFileWriter, FsProjectFiles, LockFileCoordinator, SystemClock};
pub use git::GitCli;
pub use github_workflow::FsWorkflowStore;
pub use hash::Xxh3Hasher;
pub use hooks::FsHookStore;
pub use markdown::PulldownMarkdownCodec;
pub use packet::{FsPacketInput, JsonPacketCodec};
pub use repository_ignore::GixRepositoryIgnore;
pub use skill::FsSkillStore;
pub use state::{LockStateInspector, LockStateStore};
