//! Memoria's integration tests, built as one test binary.
//!
//! Each module was a separate test binary before. One binary links once,
//! instead of once for each file, so a test build needs a fraction of the
//! disk space and time. `cargo nextest` still runs every test in its own
//! process. Run one module with `cargo nextest run --test integration
//! agent_hooks::` or `cargo test --test integration agent_hooks::`.

mod common;

mod agent;
mod agent_hooks;
mod configuration;
mod cookbook_beside_code;
mod cookbook_central_docs;
mod cookbook_readme_tree;
mod coverage_resume;
mod document_reviews;
mod documents;
mod edges;
mod git_batching;
mod github_workflows;
mod guidance;
mod handoffs;
mod integrations;
mod packets;
mod parallel;
mod portability;
mod quiet_output;
mod repairs;
mod repairs10;
mod repairs11;
mod repairs12;
mod repairs13;
mod repairs14;
mod repairs15;
mod repairs2;
mod repairs3;
mod repairs4;
mod repairs5;
mod repairs6;
mod repairs7;
mod repairs8;
mod repairs9;
mod review_context;
mod save_ack;
mod section_patterns;
mod sections;
mod state_format;
mod upgrade;
mod usability;
mod workflow;
