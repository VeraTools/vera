//! Advertised MCP tools and their input schemas.

use serde_json::Value;

use crate::protocol::ToolDefinition;

/// Git-scope filter properties shared by tool schemas: `changed`, `since`,
/// `base`. `what` names the restricted operation (e.g. "search").
fn git_scope_properties(what: &str) -> serde_json::Map<String, Value> {
    serde_json::json!({
        "changed": {
            "type": "boolean",
            "description": format!("Restrict {what} to modified, staged, and untracked files.")
        },
        "since": {
            "type": "string",
            "description": format!("Restrict {what} to files changed since the given revision.")
        },
        "base": {
            "type": "string",
            "description": format!("Restrict {what} to files changed since merge-base(HEAD, revision).")
        }
    })
    .as_object()
    .expect("git scope properties")
    .clone()
}

/// Merge the shared git-scope properties into a tool schema's `properties`.
fn with_git_scope(mut schema: Value, what: &str) -> Value {
    schema
        .pointer_mut("/properties")
        .and_then(Value::as_object_mut)
        .expect("tool schema properties")
        .extend(git_scope_properties(what));
    schema
}

/// Shared `scope` filter property (source/docs/runtime corpus selection).
fn scope_prop() -> Value {
    serde_json::json!({
        "type": "string",
        "enum": ["source", "docs", "runtime", "all"],
        "description": "Coarse corpus scope. Defaults to source-first behavior."
    })
}

/// Shared `include_generated` filter property.
fn include_generated_prop() -> Value {
    serde_json::json!({
        "type": "boolean",
        "description": "Include generated or minified files such as dist bundles."
    })
}

/// Shared `path` property naming the project directory.
fn project_path_prop() -> Value {
    serde_json::json!({
        "type": "string",
        "description": "Path to the project directory (default: current dir)"
    })
}

/// Shared `limit` property; `what` is the counted noun ("results", "matches").
fn limit_prop(what: &str, default: usize) -> Value {
    serde_json::json!({
        "type": "integer",
        "description": format!("Maximum number of {what} (default: {default})")
    })
}

/// Return the list of tools the server advertises.
pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "search_code".to_string(),
            description: "Search the indexed codebase using hybrid BM25+vector \
                          retrieval with cross-encoder reranking. Returns ranked \
                          code snippets with file paths, line numbers, and content.\n\
                          \n\
                          WHEN TO USE: conceptual or behavioral queries (\"how is auth handled\", \
                          \"error retry logic\", \"database connection pooling\"). Understands \
                          synonyms and related concepts.\n\
                          WHEN NOT TO USE: exact string matching, regex patterns, or \
                          import statements. Use regex_search for those.\n\
                          \n\
                          TIPS: Use 2-3 varied queries to capture different aspects of what \
                          you are looking for. Set intent to describe your higher-level goal \
                          for better reranking."
                .to_string(),
            input_schema: with_git_scope(serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (keyword or natural language)"
                    },
                    "queries": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Multiple search queries to run in parallel and merge. Use 2-3 varied queries to capture different aspects (e.g., ['OAuth token refresh', 'JWT expiry handling', 'auth middleware']). Results are deduplicated and reranked."
                    },
                    "intent": {
                        "type": "string",
                        "description": "Higher-level goal for reranking (e.g., 'find where auth tokens are validated and refreshed'). Improves precision when the query is ambiguous."
                    },
                    "lang": {
                        "type": "string",
                        "description": "Filter by programming language (e.g., rust, python)"
                    },
                    "path": {
                        "type": ["string", "array"],
                        "items": {"type": "string"},
                        "description": "Filter by file path glob, as a string or an array of strings. Repeated patterns use OR semantics (e.g., src/**/*.rs)"
                    },
                    "symbol_type": {
                        "type": "string",
                        "description": "Filter by symbol type (function, struct, class, etc.)"
                    },
                    "scope": scope_prop(),
                    "include_generated": include_generated_prop(),
                    "limit": limit_prop("results", 5),
                    "compact": {
                        "type": "boolean",
                        "description": "Return only function/class signatures (omit bodies). Use for broad exploration; fits more results in fewer tokens."
                    }
                },
                "anyOf": [{"required": ["query"]}, {"required": ["queries"]}]
            }), "search"),
        },
        ToolDefinition {
            name: "get_stats".to_string(),
            description: "Get index statistics: file count, chunk count, index size, \
                          and language breakdown."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": project_path_prop()
                }
            }),
        },
        ToolDefinition {
            name: "get_overview".to_string(),
            description: "Get architecture overview of the indexed project: languages, \
                          directories, entry points, symbol types, complexity hotspots, \
                          and detected project conventions (frameworks, patterns, config files). \
                          Useful for onboarding and understanding project structure."
                .to_string(),
            input_schema: with_git_scope(serde_json::json!({
                "type": "object",
                "properties": {
                    "path": project_path_prop(),
                }
            }), "overview"),
        },
        ToolDefinition {
            name: "regex_search".to_string(),
            description: "Search indexed files using a regex pattern. Returns matches \
                          with surrounding context lines.\n\
                          \n\
                          WHEN TO USE: exact string matching, regex patterns, import statements, \
                          TODOs, specific syntax, or known identifiers.\n\
                          WHEN NOT TO USE: conceptual or behavioral queries. Use search_code \
                          for those."
                .to_string(),
            input_schema: with_git_scope(serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for"
                    },
                    "limit": limit_prop("matches", 20),
                    "ignore_case": {
                        "type": "boolean",
                        "description": "Case-insensitive matching (default: false)"
                    },
                    "context": {
                        "type": "integer",
                        "description": "Context lines before and after each match (default: 2)"
                    },
                    "scope": scope_prop(),
                    "include_generated": include_generated_prop(),
                    "compact": {
                        "type": "boolean",
                        "description": "Return only function/class signatures (omit bodies). Use for broad exploration."
                    }
                },
                "required": ["pattern"]
            }), "regex search"),
        },
        ToolDefinition {
            name: "structural_search".to_string(),
            description: "Run agent-oriented structural search intents over indexed code.\n\
                          \n\
                          WHEN TO USE: symbol definitions, env var reads, \
                          HTTP route handlers, SQL execution sites, or explicit \
                          implementations/conformances/inheritance declarations.\n\
                          WHEN NOT TO USE: conceptual behavior queries or exact caller/callee \
                          lookups. Use search_code or find_references for those."
                .to_string(),
            input_schema: with_git_scope(serde_json::json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["definitions", "env_reads", "route_handlers", "sql_queries", "implementations"],
                        "description": "Structural intent to run."
                    },
                    "query": {
                        "type": "string",
                        "description": "Required for definitions and implementation lookups. Optional for env_reads to narrow to one env var. Rejected by route_handlers and sql_queries, which cannot narrow by term."
                    },
                    "lang": {
                        "type": "string",
                        "description": "Filter by programming language (e.g., rust, python)"
                    },
                    "path": {
                        "type": ["string", "array"],
                        "items": {"type": "string"},
                        "description": "Filter by file path glob, as a string or an array of strings. Repeated patterns use OR semantics (e.g., src/**/*.rs)"
                    },
                    "symbol_type": {
                        "type": "string",
                        "description": "Filter by enclosing symbol type (function, class, method, etc.)"
                    },
                    "scope": scope_prop(),
                    "include_generated": include_generated_prop(),
                    "limit": limit_prop("results", 20),
                    "compact": {
                        "type": "boolean",
                        "description": "Return only function/class signatures (omit bodies)."
                    }
                },
                "required": ["kind"]
            }), "search"),
        },
        ToolDefinition {
            name: "find_references".to_string(),
            description: "Find exact callers or callees of a symbol using Vera's persisted call graph.\n\
                          \n\
                          WHEN TO USE: who calls a symbol, what a symbol calls, or when \
                          narrowing exact call relationships to a diff.\n\
                          WHEN NOT TO USE: conceptual behavior queries or heuristic structural \
                          scans. Use search_code or structural_search for those."
                .to_string(),
            input_schema: with_git_scope(serde_json::json!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "Symbol name to look up."
                    },
                    "callees": {
                        "type": "boolean",
                        "description": "Return what the symbol calls instead of who calls it."
                    },
                    "limit": limit_prop("results", 20),
                    "compact": {
                        "type": "boolean",
                        "description": "For caller lookups, return only function/class signatures."
                    }
                },
                "required": ["symbol"]
            }), "references"),
        },
        ToolDefinition {
            name: "explain_path".to_string(),
            description: "Explain why a path is or is not indexed. Returns the decisive reason such as a default exclude, .veraignore, .gitignore, binary detection, size limit, or missing file."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Repository-relative or absolute path to explain."
                    },
                    "exclude": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Extra exclusion globs to apply, matching CLI --exclude semantics."
                    },
                    "no_ignore": {
                        "type": "boolean",
                        "description": "Disable .gitignore and .veraignore parsing."
                    },
                    "no_default_excludes": {
                        "type": "boolean",
                        "description": "Disable Vera's built-in default exclusions."
                    }
                },
                "required": ["path"]
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_has_seven_tools() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 7);

        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"search_code"));
        assert!(names.contains(&"get_stats"));
        assert!(names.contains(&"get_overview"));
        assert!(names.contains(&"regex_search"));
        assert!(names.contains(&"structural_search"));
        assert!(names.contains(&"find_references"));
        assert!(names.contains(&"explain_path"));
    }

    #[test]
    fn git_scope_schema_is_only_on_overview() {
        let tools = tool_definitions();
        let get_stats = tools.iter().find(|tool| tool.name == "get_stats").unwrap();
        let get_overview = tools
            .iter()
            .find(|tool| tool.name == "get_overview")
            .unwrap();

        let stats_props = get_stats.input_schema["properties"].as_object().unwrap();
        assert!(!stats_props.contains_key("changed"));
        assert!(!stats_props.contains_key("since"));
        assert!(!stats_props.contains_key("base"));

        let overview_props = get_overview.input_schema["properties"].as_object().unwrap();
        assert!(overview_props.contains_key("changed"));
        assert!(overview_props.contains_key("since"));
        assert!(overview_props.contains_key("base"));
    }

    #[test]
    fn git_scope_descriptions_name_the_right_operation() {
        let tools = tool_definitions();
        for (tool_name, noun) in [
            ("search_code", "search"),
            ("get_overview", "overview"),
            ("regex_search", "regex search"),
            ("structural_search", "search"),
            ("find_references", "references"),
        ] {
            let tool = tools.iter().find(|tool| tool.name == tool_name).unwrap();
            let description = tool.input_schema["properties"]["changed"]["description"]
                .as_str()
                .unwrap();
            assert!(
                description.starts_with(&format!("Restrict {noun} to")),
                "{tool_name} changed description should name '{noun}': {description}"
            );
        }
    }

    #[test]
    fn tool_definitions_have_valid_schemas() {
        let tools = tool_definitions();
        for tool in &tools {
            let schema = &tool.input_schema;
            assert_eq!(schema["type"], "object", "tool {} schema type", tool.name);
        }
    }

    #[test]
    fn regex_search_schema_stays_minimal() {
        let tools = tool_definitions();
        let regex_search = tools
            .iter()
            .find(|tool| tool.name == "regex_search")
            .unwrap();
        let properties = regex_search.input_schema["properties"].as_object().unwrap();

        assert!(properties.contains_key("pattern"));
        assert!(properties.contains_key("scope"));
        assert!(properties.contains_key("include_generated"));
        assert!(!properties.contains_key("lang"));
        assert!(!properties.contains_key("path"));
        assert!(!properties.contains_key("symbol_type"));
    }

    #[test]
    fn structural_search_schema_exposes_kind_and_git_scope() {
        let tools = tool_definitions();
        let structural = tools
            .iter()
            .find(|tool| tool.name == "structural_search")
            .unwrap();
        let properties = structural.input_schema["properties"].as_object().unwrap();

        assert!(properties.contains_key("kind"));
        assert!(properties.contains_key("query"));
        assert!(properties.contains_key("changed"));
        assert!(properties.contains_key("since"));
        assert!(properties.contains_key("base"));
        assert!(
            !properties
                .get("kind")
                .and_then(|kind| kind.get("enum"))
                .and_then(|value| value.as_array())
                .unwrap()
                .iter()
                .any(|value| value == "calls")
        );
    }

    #[test]
    fn references_schema_exposes_symbol_and_git_scope() {
        let tools = tool_definitions();
        let refs = tools
            .iter()
            .find(|tool| tool.name == "find_references")
            .unwrap();
        let properties = refs.input_schema["properties"].as_object().unwrap();

        assert!(properties.contains_key("symbol"));
        assert!(properties.contains_key("callees"));
        assert!(properties.contains_key("changed"));
        assert!(properties.contains_key("since"));
        assert!(properties.contains_key("base"));
    }
}
