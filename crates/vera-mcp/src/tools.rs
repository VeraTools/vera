//! MCP tool definitions and handler dispatch.
//!
//! Defines the tools that the Vera MCP server exposes:
//! - `search_code` — search indexed codebase (auto-indexes and watches on first use)
//! - `get_stats` — retrieve index statistics
//! - `get_overview` — architecture overview for agent onboarding
//! - `regex_search` — regex search over indexed files
//! - `structural_search` — agent-oriented structural search intents
//! - `find_references` — exact callers or callees from the persisted call graph
//! - `explain_path` — explain why a path is or is not indexed

mod handlers;
mod runtime;
mod schemas;

pub use handlers::handle_tool_call;
pub use schemas::tool_definitions;
