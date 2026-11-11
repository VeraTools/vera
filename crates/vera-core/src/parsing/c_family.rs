//! Conservative recovery for C-family syntax in source-search indexes.

use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::{Parser, Tree};

use crate::types::Language;

/// Retry common C-family annotations and ambiguous `.h` files without changing
/// indexed text. Keep a successful C parse; use C++ only for a clean retry that
/// contains a C++-specific construct.
pub(super) fn recover(
    parser: &mut Parser,
    source: &str,
    file_path: &str,
    language: Language,
    tree: Tree,
) -> Option<(Tree, Language)> {
    if !matches!(language, Language::C | Language::Cpp) || !tree.root_node().has_error() {
        return Some((tree, language));
    }
    let tree = recover_annotations(parser, source, tree)?;
    if language != Language::C || !tree.root_node().has_error() {
        return Some((tree, language));
    }
    let extension = file_path.rsplit('.').next().unwrap_or_default();
    if !extension.eq_ignore_ascii_case("h") {
        return Some((tree, language));
    }
    parser
        .set_language(&super::languages::tree_sitter_grammar(Language::Cpp)?)
        .ok()?;
    let candidate = parser.parse(source, None)?;
    let candidate = recover_annotations(parser, source, candidate)?;
    if !candidate.root_node().has_error() && has_cpp_construct(&candidate) {
        Some((candidate, Language::Cpp))
    } else {
        Some((tree, language))
    }
}

/// Ignore only recognized calling-convention tokens that the grammar itself
/// placed in an ERROR node. Successful parses and ordinary identifiers are
/// untouched. The recovery is used only when the entire retry parses cleanly.
fn recover_annotations(parser: &mut Parser, source: &str, tree: Tree) -> Option<Tree> {
    if !tree.root_node().has_error() {
        return Some(tree);
    }
    let mut masked = source.as_bytes().to_vec();
    let guards_changed = mask_linkage_guards(source, &tree, &mut masked);
    let mut annotations_changed = false;
    // Removing a linkage wrapper can expose annotation errors that were
    // previously swallowed by a file-wide ERROR node.
    let guard_tree = if guards_changed {
        parser.parse(&masked, None)
    } else {
        None
    };
    let scan_tree = guard_tree.as_ref().unwrap_or(&tree);
    let mut cursor = scan_tree.walk();
    'walk: loop {
        let node = cursor.node();
        if node.is_error() {
            let text = &source[node.byte_range()];
            let trimmed = text.trim_start();
            let token_end = trimmed
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(trimmed.len());
            let token = &trimmed[..token_end];
            if matches!(
                token,
                "WINAPI" | "CALLBACK" | "APIENTRY" | "NTAPI" | "WINAPIV" | "PASCAL"
            ) && trimmed[token_end..]
                .chars()
                .all(|c| c.is_ascii_whitespace() || c == '*')
                && is_code_offset(scan_tree, node.start_byte())
            {
                let start = node.start_byte() + text.len() - trimmed.len();
                masked[start..start + token.len()].fill(b' ');
                annotations_changed = true;
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    drop(cursor);
    if !guards_changed && !annotations_changed {
        return Some(tree);
    }
    let candidate = if annotations_changed {
        parser.parse(&masked, None)?
    } else {
        guard_tree?
    };
    Some(if candidate.root_node().has_error() {
        tree
    } else {
        candidate
    })
}

/// Match only the conventional, three-line C++ linkage wrappers. This is not
/// a preprocessor: no conditional branch containing declarations is discarded.
fn mask_linkage_guards(source: &str, tree: &Tree, masked: &mut [u8]) -> bool {
    static GUARD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r#"(?m)^[\t ]*#[\t ]*(?:ifdef[\t ]+__cplusplus|if[\t ]+defined[\t ]*(?:\([\t ]*__cplusplus[\t ]*\)|[\t ]+__cplusplus))[\t ]*\r?\n"#,
            r#"[\t ]*(?P<body>extern[\t ]+"C"[\t ]*\{|\})(?:[\t ]*(?://[^\r\n]*|/\*(?:[^*\r\n]|\*+[^*/\r\n])*\*+/))?[\t ]*\r?\n"#,
            r#"[\t ]*#[\t ]*endif[\t ]*(?://[^\r\n]*|/\*(?:[^*\r\n]|\*+[^*/\r\n])*\*+/)?[\t ]*\r?$"#,
        )).expect("valid linkage guard regex")
    });
    let mut openings = Vec::new();
    let mut changed = false;
    for captures in GUARD.captures_iter(source) {
        let whole = captures.get(0).expect("whole guard");
        let body = captures.name("body").expect("guard body");
        let hash = whole.start() + whole.as_str().find('#').expect("guard directive");
        if !is_code_offset(tree, hash) {
            continue;
        }
        if body.as_str().starts_with("extern") {
            openings.push((whole.range(), body.end() - 1));
        } else if let Some((open_range, open_brace)) = openings.pop()
            && matching_braces(tree, open_brace, body.start())
        {
            for range in [open_range, whole.range()] {
                for byte in &mut masked[range] {
                    if !matches!(*byte, b'\r' | b'\n') {
                        *byte = b' ';
                    }
                }
            }
            changed = true;
        }
    }
    changed
}

/// Require the two wrapper braces to match lexically; skip comments, string
/// literals and synthetic missing tokens by inspecting actual brace leaves.
fn matching_braces(tree: &Tree, opening: usize, closing: usize) -> bool {
    let mut depth = 0usize;
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if node.start_byte() >= opening && node.start_byte() <= closing && !node.is_missing() {
            match node.kind() {
                "{" => depth += 1,
                "}" => {
                    let Some(next) = depth.checked_sub(1) else {
                        return false;
                    };
                    depth = next;
                    if depth == 0 {
                        return node.start_byte() == closing;
                    }
                }
                _ => {}
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return false;
            }
        }
    }
}

fn is_code_offset(tree: &Tree, offset: usize) -> bool {
    let mut node = tree
        .root_node()
        .descendant_for_byte_range(offset, offset + 1);
    while let Some(current) = node {
        if matches!(
            current.kind(),
            "comment" | "string_literal" | "raw_string_literal" | "char_literal" | "preproc_arg"
        ) {
            return false;
        }
        node = current.parent();
    }
    true
}

fn has_cpp_construct(tree: &Tree) -> bool {
    let mut cursor = tree.walk();
    loop {
        if matches!(
            cursor.node().kind(),
            "alias_declaration"
                | "class_specifier"
                | "namespace_definition"
                | "template_declaration"
                | "reference_declarator"
                | "qualified_identifier"
                | "field_initializer_list"
        ) {
            return true;
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::IndexingConfig;
    use crate::parsing::parse_file_with_diagnostics;
    use crate::types::Language;

    #[test]
    fn cpp_header_uses_cpp_symbols_and_preserves_source() {
        let source = "template<class T> class Box { public: T value; };\n";
        let (chunks, _, diagnostics) =
            parse_file_with_diagnostics(source, "box.h", Language::C, &IndexingConfig::default())
                .unwrap();
        assert!(!diagnostics.tree_has_error);
        assert!(chunks.iter().all(|chunk| chunk.language == Language::Cpp));
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.symbol_name.as_deref() == Some("Box"))
        );
        for chunk in chunks {
            let expected = source
                .lines()
                .skip(chunk.line_start as usize - 1)
                .take((chunk.line_end - chunk.line_start + 1) as usize)
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(chunk.content, expected);
        }
    }

    #[test]
    fn valid_c_header_keeps_c_language() {
        let source = "struct item { int value; };\nint read_item(struct item *item);\n";
        let (chunks, _, diagnostics) =
            parse_file_with_diagnostics(source, "item.h", Language::C, &IndexingConfig::default())
                .unwrap();
        assert!(!diagnostics.tree_has_error);
        assert!(chunks.iter().all(|chunk| chunk.language == Language::C));
    }

    #[test]
    fn cpp_syntax_does_not_change_explicit_c_source_language() {
        let (_, _, diagnostics) = parse_file_with_diagnostics(
            "template<class T> class Box {};",
            "box.c",
            Language::C,
            &IndexingConfig::default(),
        )
        .unwrap();
        assert!(diagnostics.tree_has_error);
    }

    #[test]
    fn malformed_header_keeps_diagnostics() {
        let (_, _, diagnostics) = parse_file_with_diagnostics(
            "template<class T> class Box {",
            "box.h",
            Language::C,
            &IndexingConfig::default(),
        )
        .unwrap();
        assert!(diagnostics.tree_has_error);
    }
    #[test]
    fn winapi_recovery_preserves_content_lines_and_calls() {
        for language in [Language::C, Language::Cpp] {
            let source = "// Unicode: \u{03bb}\r\n#define WINAPI __stdcall\r\nunsigned long WINAPI worker(void *arg) { return helper(); }\r\n";
            let (chunks, refs, diagnostics) = parse_file_with_diagnostics(
                source,
                "worker.cpp",
                language,
                &IndexingConfig::default(),
            )
            .unwrap();
            assert!(!diagnostics.tree_has_error);
            let worker = chunks
                .iter()
                .find(|c| c.symbol_name.as_deref() == Some("worker"))
                .unwrap();
            assert!(worker.content.contains("WINAPI worker"));
            assert_eq!(worker.line_end, 3);
            assert!(source.len() > source.chars().count());
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(&super::super::languages::tree_sitter_grammar(language).unwrap())
                .unwrap();
            let (tree, effective_language) =
                super::super::parse_tree(&mut parser, source, "worker.cpp", language).unwrap();
            let symbols = super::super::extractor::extract_symbols(
                &tree,
                source.as_bytes(),
                effective_language,
            );
            let symbol = symbols
                .iter()
                .find(|s| s.name.as_deref() == Some("worker"))
                .unwrap();
            assert_eq!(symbol.start_byte, source.find("unsigned long").unwrap());
            assert_eq!(symbol.end_byte, source.rfind('}').unwrap() + 1);
            assert!(refs.iter().any(|r| r.callee == "helper" && r.line == 3));
        }
    }

    #[test]
    fn calling_conventions_in_function_pointer_types_are_recovered() {
        let source = "typedef long (NTAPI *Handler)(void *arg);\n";
        let (_, _, diagnostics) = parse_file_with_diagnostics(
            source,
            "handler.h",
            Language::C,
            &IndexingConfig::default(),
        )
        .unwrap();
        assert!(!diagnostics.tree_has_error);
    }

    #[test]
    fn annotations_do_not_hide_unrelated_syntax_errors() {
        let source = "int WINAPI worker(void) { return 0; }\nint broken( {\n";
        let (_, _, diagnostics) = parse_file_with_diagnostics(
            source,
            "worker.c",
            Language::C,
            &IndexingConfig::default(),
        )
        .unwrap();
        assert!(diagnostics.tree_has_error);
    }

    #[test]
    fn ordinary_winapi_identifiers_and_strings_are_unchanged() {
        let source = "int WINAPI(void) { return 1; }\nconst char *s = \"WINAPI CALLBACK\";\nint caller(void) { return WINAPI(); }\n";
        let (chunks, refs, diagnostics) = parse_file_with_diagnostics(
            source,
            "ordinary.c",
            Language::C,
            &IndexingConfig::default(),
        )
        .unwrap();
        assert!(!diagnostics.tree_has_error);
        assert!(
            chunks
                .iter()
                .any(|c| c.symbol_name.as_deref() == Some("WINAPI"))
        );
        assert!(refs.iter().any(|r| r.callee == "WINAPI"));
    }
    #[test]
    fn conditional_linkage_guards_preserve_c_symbols_and_calls() {
        for guard in [
            "#ifdef __cplusplus",
            "#if defined(__cplusplus)",
            "#if defined __cplusplus",
        ] {
            let source = format!(
                "{guard}\r\nextern \"C\" {{\r\n#endif\r\nint WINAPI worker(void) {{ return helper(); }}\r\n{guard}\r\n}} /* extern C */\r\n#endif\r\n"
            );
            let (chunks, refs, diagnostics) = parse_file_with_diagnostics(
                &source,
                "api.h",
                Language::C,
                &IndexingConfig::default(),
            )
            .unwrap();
            assert!(!diagnostics.tree_has_error, "{guard}");
            let worker = chunks
                .iter()
                .find(|c| c.symbol_name.as_deref() == Some("worker"))
                .unwrap();
            assert_eq!(worker.language, Language::C);
            assert_eq!(worker.line_start, 4);
            assert_eq!(
                worker.content,
                "int WINAPI worker(void) { return helper(); }"
            );
            assert!(refs.iter().any(|r| r.callee == "helper" && r.line == 4));
        }
    }

    #[test]
    fn linkage_guard_recovery_does_not_hide_broken_source() {
        let source = "#ifdef __cplusplus\nextern \"C\" {\n#endif\nint broken( {\n#ifdef __cplusplus\n}\n#endif\n";
        let (_, _, diagnostics) = parse_file_with_diagnostics(
            source,
            "broken.h",
            Language::C,
            &IndexingConfig::default(),
        )
        .unwrap();
        assert!(diagnostics.tree_has_error);
    }

    #[test]
    fn guards_in_comments_are_not_masked() {
        let source = "/*\n#ifdef __cplusplus\nextern \"C\" {\n#endif\n#ifdef __cplusplus\n}\n#endif\n*/\nint WINAPI worker(void) { return 0; }\n";
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&super::super::languages::tree_sitter_grammar(Language::C).unwrap())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        let mut masked = source.as_bytes().to_vec();
        assert!(!super::mask_linkage_guards(source, &tree, &mut masked));
        assert_eq!(masked, source.as_bytes());
    }

    #[test]
    fn unmatched_and_else_guards_are_not_masked() {
        for source in [
            "#ifdef __cplusplus\nextern \"C\" {\n#endif\nvoid sample(void);\n",
            "#ifdef __cplusplus\nextern \"C\" {\n#else\nvoid c_only(void);\n#endif\nvoid sample(void);\n#ifdef __cplusplus\n}\n#endif\n",
        ] {
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(&super::super::languages::tree_sitter_grammar(Language::C).unwrap())
                .unwrap();
            let tree = parser.parse(source, None).unwrap();
            let mut masked = source.as_bytes().to_vec();
            assert!(!super::mask_linkage_guards(source, &tree, &mut masked));
        }
    }
    #[test]
    fn all_supported_conventions_and_cpp_header_recovery_compose() {
        for convention in [
            "WINAPI", "CALLBACK", "APIENTRY", "NTAPI", "WINAPIV", "PASCAL",
        ] {
            let source = format!(
                "class Worker {{ public: int {convention} run() {{ return helper(); }} }};\n"
            );
            let (chunks, refs, diagnostics) = parse_file_with_diagnostics(
                &source,
                "worker.h",
                Language::C,
                &IndexingConfig::default(),
            )
            .unwrap();
            assert!(!diagnostics.tree_has_error, "{convention}");
            assert!(chunks.iter().all(|chunk| chunk.language == Language::Cpp));
            assert!(refs.iter().any(|r| r.callee == "helper"));
        }
    }

    #[test]
    fn compact_signature_retains_convention_but_omits_body() {
        let source = "int WINAPI worker(void) { return helper(); }";
        let signature =
            crate::parsing::signatures::extract_signature_for_path(source, Language::C, "worker.c");
        assert_eq!(signature, "int WINAPI worker(void) { ... }");
    }

    #[test]
    fn references_only_api_uses_the_same_recovery() {
        let refs = crate::parsing::parse_and_extract_references(
            "int WINAPI worker(void) { return helper(); }",
            Language::C,
        );
        assert!(
            refs.iter()
                .any(|r| r.callee == "helper" && r.caller.as_deref() == Some("worker"))
        );
    }

    #[test]
    fn nested_braces_and_utf8_literals_do_not_confuse_linkage_matching() {
        let source = "#ifdef __cplusplus\nextern \"C\" {\n#endif\nconst char *text = \"\u{03bb} } {\";\nint f(void) { if (1) { return 1; } return 0; }\n#ifdef __cplusplus\n}\n#endif\n";
        let (chunks, _, diagnostics) = parse_file_with_diagnostics(
            source,
            "nested.h",
            Language::C,
            &IndexingConfig::default(),
        )
        .unwrap();
        assert!(!diagnostics.tree_has_error);
        assert!(
            chunks
                .iter()
                .any(|c| c.symbol_name.as_deref() == Some("f") && c.line_start == 5)
        );
    }
    #[test]
    fn text_between_comments_is_not_hidden_by_linkage_recovery() {
        let source = "#ifdef __cplusplus\nextern \"C\" { /* first */ invalid_token /* second */\n#endif\nvoid sample(void);\n#ifdef __cplusplus\n}\n#endif\n";
        let (_, _, diagnostics) = parse_file_with_diagnostics(
            source,
            "broken.h",
            Language::C,
            &IndexingConfig::default(),
        )
        .unwrap();
        assert!(diagnostics.tree_has_error);
    }
    #[test]
    fn nested_conditional_linkage_guards_preserve_all_symbols() {
        let open = "#ifdef __cplusplus\nextern \"C\" {\n#endif\n";
        let close = "#ifdef __cplusplus\n}\n#endif\n";
        let source = format!(
            "{open}int outer(void) {{ return first(); }}\n{open}int inner(void) {{ return second(); }}\n{close}int after(void) {{ return third(); }}\n{close}"
        );
        let (chunks, refs, diagnostics) = parse_file_with_diagnostics(
            &source,
            "nested.h",
            Language::C,
            &IndexingConfig::default(),
        )
        .unwrap();
        assert!(!diagnostics.tree_has_error);
        for (name, line, callee) in [
            ("outer", 4, "first"),
            ("inner", 8, "second"),
            ("after", 12, "third"),
        ] {
            assert!(
                chunks
                    .iter()
                    .any(|c| c.symbol_name.as_deref() == Some(name) && c.line_start == line)
            );
            assert!(refs.iter().any(|r| r.callee == callee
                && r.caller.as_deref() == Some(name)
                && r.line == line));
        }
    }

    #[test]
    fn path_aware_references_match_cpp_header_indexing() {
        let source = "template<class T> class Box { public: int run() { return helper(); } };\n";
        let (_, expected, diagnostics) =
            parse_file_with_diagnostics(source, "box.h", Language::C, &IndexingConfig::default())
                .unwrap();
        assert!(!diagnostics.tree_has_error);
        let actual =
            crate::parsing::parse_and_extract_references_for_path(source, "box.h", Language::C);
        assert!(!actual.is_empty());
        let edges = |refs: Vec<crate::parsing::references::RawReference>| {
            refs.into_iter()
                .map(|r| (r.callee, r.caller, r.qualifier, r.line))
                .collect::<Vec<_>>()
        };
        assert_eq!(edges(actual), edges(expected));
    }

    #[test]
    fn path_aware_references_respect_tsx_dialect() {
        let source = "function Panel() { return <Button onClick={handleClick()} />; }";
        let (_, expected, _) = parse_file_with_diagnostics(
            source,
            "Panel.tsx",
            Language::TypeScript,
            &IndexingConfig::default(),
        )
        .unwrap();
        let actual = crate::parsing::parse_and_extract_references_for_path(
            source,
            "Panel.tsx",
            Language::TypeScript,
        );
        assert!(actual.iter().any(|r| r.callee == "handleClick"));
        assert_eq!(
            actual
                .iter()
                .map(|r| (&r.callee, &r.caller, r.line))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|r| (&r.callee, &r.caller, r.line))
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn cpp_alias_header_uses_cpp_language() {
        let source = "using Alias = int *;\n";
        let (chunks, _, diagnostics) =
            parse_file_with_diagnostics(source, "alias.h", Language::C, &IndexingConfig::default())
                .unwrap();
        assert!(!diagnostics.tree_has_error);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|chunk| chunk.language == Language::Cpp));
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.content.contains("using Alias = int *;"))
        );
    }
}
