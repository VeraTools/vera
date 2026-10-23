//! CLI command implementations.
//!
//! Each subcommand is implemented in its own module to keep files focused
//! and under the 500-line budget.

pub mod agent;
pub mod config;
pub mod doctor;
pub mod estimate;
pub mod explain_path;
pub mod grep;
pub mod index;
pub mod mcp;
pub mod overview;
pub mod references;
pub mod repair;
pub mod search;
pub mod serve;
pub mod setup;
pub mod stats;
pub mod structural;
pub mod uninstall;
pub mod update;
pub mod upgrade;
pub mod watch;
