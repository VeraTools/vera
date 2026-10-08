//! Result formatting, index summaries, and progress rendering.

use std::sync::{Arc, Mutex};

use vera_core::indexing::progress::EmbedDisplay;
use vera_core::presentation::{CompactResult, truncate_to_budget};

use super::runtime::is_cancel_error;

// ── Shared progress rendering (deduplicated for index + update) ─────────

/// Render an embed progress display into the shared spinner/bar widgets.
///
/// This is the deduplicated core of `vera index` and `vera update` progress
/// rendering: indeterminate → spinner, determinate → bar. Both commands call
/// this so the tracker + embed_spinner/embed_bar + ParsingDone transition
/// cannot drift. The `display` comes from `HonestProgressTracker` or
/// `UpdateProgressTracker`; the widget handling is identical.
pub fn render_embed_display(
    display: Option<EmbedDisplay>,
    embed_spinner: &Arc<Mutex<Option<Arc<cliclack::ProgressBar>>>>,
    embed_bar: &Arc<Mutex<Option<Arc<cliclack::ProgressBar>>>>,
    multi: &cliclack::MultiProgress,
) {
    match display {
        Some(EmbedDisplay::Indeterminate { done }) => {
            let mut guard = embed_spinner.lock().unwrap();
            if guard.is_none() {
                let w = Arc::new(multi.add(cliclack::spinner()));
                w.start(format!("Generating embeddings ({} chunks so far)", done));
                *guard = Some(w);
            } else if let Some(w) = guard.as_ref() {
                w.set_message(format!("Generating embeddings ({} chunks so far)", done));
            }
        }
        Some(EmbedDisplay::Determinate { done, total }) => {
            // If an indeterminate spinner is still around (race where
            // ParsingDone and first determinate embedding interleave), stop
            // it before showing the bar.
            if let Some(spinner_widget) = embed_spinner.lock().unwrap().take() {
                spinner_widget.stop(format!(
                    "Parsed into {total} chunks — finalizing embeddings..."
                ));
            }
            let mut guard = embed_bar.lock().unwrap();
            if guard.is_none() {
                let w = Arc::new(multi.add(cliclack::progress_bar(total as u64)));
                w.start(format!("Generating embeddings ({}/{})", done, total));
                w.set_position(done as u64);
                *guard = Some(w);
            } else if let Some(w) = guard.as_ref() {
                w.set_position(done as u64);
                w.set_message(format!("Generating embeddings ({}/{})", done, total));
            }
        }
        Some(EmbedDisplay::Done { .. }) | None => {}
    }
}

/// Stop the embed spinner on `ParsingDone` transition, if present.
///
/// Shared helper for the ParsingDone branch: both index and update stop the
/// spinner with a "finalizing embeddings..." message so the determinate bar
/// can take over without a lingering line.
pub fn stop_embed_spinner_on_parsing_done(
    embed_spinner: &Arc<Mutex<Option<Arc<cliclack::ProgressBar>>>>,
    message: String,
) {
    if let Some(spinner_widget) = embed_spinner.lock().unwrap().take() {
        spinner_widget.stop(message);
    }
}

/// Handle an `EmbeddingDone` display: stop whichever embed widget is active
/// and show the final "Generated N embeddings" message. For tiny repos where
/// no widget was ever created, a short-lived bar is shown. Shared by index
/// and update.
///
/// The fallback bar is only created when `count > 0`: for an up-to-date or
/// zero-chunk update `EmbeddingDone { count: 0 }` is emitted without any prior
/// `EmbeddingProgress`, and creating a transient "Generated 0 embeddings" bar
/// would flash a misleading widget before the next spinner.
pub fn handle_embedding_done(
    display: Option<EmbedDisplay>,
    embed_spinner: &Arc<Mutex<Option<Arc<cliclack::ProgressBar>>>>,
    embed_bar: &Arc<Mutex<Option<Arc<cliclack::ProgressBar>>>>,
    multi: &cliclack::MultiProgress,
) {
    if let Some(EmbedDisplay::Done { count }) = display {
        if let Some(bar) = embed_bar.lock().unwrap().take() {
            bar.stop(format!("Generated {} embeddings", count));
        } else if let Some(spinner_widget) = embed_spinner.lock().unwrap().take() {
            spinner_widget.stop(format!("Generated {} embeddings", count));
        } else if count > 0 {
            let w = multi.add(cliclack::progress_bar(count as u64));
            w.start(format!("Generated {} embeddings", count));
            w.stop(format!("Generated {} embeddings", count));
        }
    }
}

/// Finalize the progress UI after the task completes.
///
/// Shared cancel/error/success handling: cancellation is detected via the
/// typed `is_cancel_error` helper (not substring), then widgets are
/// cancelled, errored, or stopped accordingly. Both `vera index` and
/// `vera update` call this so the outcome cannot diverge.
pub fn finalize_progress_ui<T>(
    result: &anyhow::Result<T>,
    parse_spinner: &Arc<cliclack::ProgressBar>,
    embed_spinner: &Arc<Mutex<Option<Arc<cliclack::ProgressBar>>>>,
    embed_bar: &Arc<Mutex<Option<Arc<cliclack::ProgressBar>>>>,
    multi: &cliclack::MultiProgress,
) {
    let is_cancel = result.as_ref().is_err_and(is_cancel_error);
    if is_cancel {
        if let Some(bar) = embed_bar.lock().unwrap().take() {
            bar.cancel("Cancelled");
        }
        if let Some(spinner_widget) = embed_spinner.lock().unwrap().take() {
            spinner_widget.cancel("Cancelled");
        }
        parse_spinner.cancel("Cancelled");
        multi.cancel();
    } else if let Err(err) = result {
        let msg = err.to_string();
        if let Some(bar) = embed_bar.lock().unwrap().take() {
            bar.error(format!("Failed: {msg}"));
        }
        if let Some(spinner_widget) = embed_spinner.lock().unwrap().take() {
            spinner_widget.error(format!("Failed: {msg}"));
        }
        parse_spinner.error(format!("Failed: {msg}"));
        multi.error(&msg);
    } else {
        multi.stop();
    }
}

/// Output search results with a total character budget.
///
/// Priority: `--json` compact JSON > `--raw` verbose > default markdown codeblocks.
/// When `budget` is non-zero, output is truncated so it stays within the budget:
/// markdown and raw mode spend it progressively across results (lower-ranked
/// results are dropped first), JSON mode truncates the serialized document.
/// When `compact` is true, function/class bodies are stripped to show only signatures.
pub fn output_results(
    results: &[vera_core::types::SearchResult],
    json_output: bool,
    raw: bool,
    compact: bool,
    budget: usize,
) {
    use vera_core::parsing::signatures::extract_signature_for_path;

    // When compact mode is on, pre-compute signature-only content for each result.
    let compacted: Vec<String> = if compact {
        results
            .iter()
            .map(|r| extract_signature_for_path(&r.content, r.language, &r.file_path))
            .collect()
    } else {
        Vec::new()
    };
    let contents: Vec<&str> = results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            if compact {
                compacted[i].as_str()
            } else {
                r.content.as_str()
            }
        })
        .collect();

    if json_output {
        println!("{}", json_within_budget(results, &contents, budget));
    } else if raw {
        if results.is_empty() {
            println!("No results found.");
        } else {
            print!("{}", format_raw_results(results, &contents, budget));
        }
    } else {
        // Default: markdown codeblocks (most token-efficient for LLM agents).
        let mut remaining = budget;
        for (i, r) in results.iter().enumerate() {
            if budget > 0 && remaining == 0 {
                break;
            }
            if i > 0 {
                println!();
            }
            println!("```{}", result_info_line(r));
            let content = budget_slice(contents[i], budget, &mut remaining);
            print!("{}", content);
            if !content.ends_with('\n') {
                println!();
            }
            println!("```");
        }
    }
}

fn result_info_line(r: &vera_core::types::SearchResult) -> String {
    let mut info = format!("{}:{}-{}", r.file_path, r.line_start, r.line_end);
    if let (Some(stype), Some(name)) = (&r.symbol_type, r.display_name()) {
        info.push_str(&format!(" {stype}:{name}"));
    }
    info
}

/// Spend `remaining` on this chunk of content when a budget is set, returning
/// the part that fits.
fn budget_slice<'a>(
    content: &'a str,
    budget: usize,
    remaining: &mut usize,
) -> std::borrow::Cow<'a, str> {
    if budget == 0 {
        return std::borrow::Cow::Borrowed(content);
    }
    let c = truncate_to_budget(content, *remaining);
    *remaining = remaining.saturating_sub(c.len());
    c
}

fn json_results_string(results: &[vera_core::types::SearchResult], contents: &[&str]) -> String {
    let json_results: Vec<CompactResult> = results
        .iter()
        .zip(contents)
        .map(|(r, content)| {
            let mut cr = CompactResult::from_search_result(r);
            cr.content = std::borrow::Cow::Borrowed(content);
            cr
        })
        .collect();
    serde_json::to_string(&json_results)
        .unwrap_or_else(|e| format!("{{\"error\": \"failed to serialize: {e}\"}}"))
}

/// Serialize results so the document stays valid JSON within `budget`.
///
/// Cutting the serialized text at a byte offset splits string literals and
/// leaves arrays unclosed, which breaks every programmatic consumer. Whole
/// results are dropped instead, and a lone oversized result has its content
/// shortened before serialization.
fn json_within_budget(
    results: &[vera_core::types::SearchResult],
    contents: &[&str],
    budget: usize,
) -> String {
    let json = json_results_string(results, contents);
    if budget == 0 || json.len() <= budget || results.is_empty() {
        return json;
    }
    for count in (1..results.len()).rev() {
        let shorter = json_results_string(&results[..count], &contents[..count]);
        if shorter.len() <= budget {
            return shorter;
        }
    }
    // One result still exceeds the budget: keep the longest content prefix
    // whose serialized form fits. Escaping makes raw byte counts unreliable,
    // so measure the serialized document itself.
    let head = &results[..1];
    let render = |allowed: usize| {
        let trimmed = truncate_to_budget(contents[0], allowed);
        Some(json_results_string(head, &[trimmed.as_ref()])).filter(|json| json.len() <= budget)
    };
    let Some(mut best) = render(0) else {
        // Metadata alone exceeds the budget.
        return "[]".to_string();
    };
    let (mut low, mut high) = (1, contents[0].len());
    while low <= high {
        let mid = low + (high - low) / 2;
        match render(mid) {
            Some(json) => {
                best = json;
                low = mid + 1;
            }
            None => high = mid - 1,
        }
    }
    best
}

/// Numbered verbose listing, one block per result. With a budget, each result's
/// content spends from a shared allowance like markdown mode does; once it runs
/// out, lower-ranked results are dropped (headers never consume budget).
fn format_raw_results(
    results: &[vera_core::types::SearchResult],
    contents: &[&str],
    budget: usize,
) -> String {
    let mut out = String::new();
    let mut remaining = budget;
    for (i, result) in results.iter().enumerate() {
        if budget > 0 && remaining == 0 {
            break;
        }
        out.push_str(&format!(
            "{}. {} (lines {}-{}, {})\n",
            i + 1,
            result.file_path,
            result.line_start,
            result.line_end,
            result.language,
        ));
        if let Some(name) = result.display_name() {
            match &result.symbol_type {
                Some(stype) => out.push_str(&format!("   {stype} {name}\n")),
                None => out.push_str(&format!("   {name}\n")),
            }
        }
        out.push_str(&format!("   score: {:.6}\n", result.score));
        let content = budget_slice(contents[i], budget, &mut remaining);
        for line in content.lines().take(3) {
            out.push_str("   │ ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// Format a byte count as a compact human-readable string (e.g. "1.2 MB").
fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1_024.0;
    const MB: f64 = 1_024.0 * KB;
    const GB: f64 = 1_024.0 * MB;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.1} KB", b / KB)
    }
}

/// Print a human-readable summary of the indexing run.
///
/// When `verbose` is true, individual file paths are listed for skipped-file
/// categories. Otherwise only counts are shown with a hint to rerun with `-v`.
pub fn print_human_summary(summary: &vera_core::indexing::IndexSummary, verbose: bool) {
    println!("Indexing complete!");
    println!();
    println!("  Files parsed:        {}", summary.files_parsed);
    println!("  Chunks created:      {}", summary.chunks_created);
    println!("  Embeddings generated: {}", summary.embeddings_generated);
    println!("  Elapsed time:        {:.2}s", summary.elapsed_secs);

    if summary.files_with_tree_sitter_errors > 0 || summary.files_using_tier0_fallback > 0 {
        println!();
        println!("  Index health:");
        if summary.files_with_tree_sitter_errors > 0 {
            println!(
                "    Tree-sitter errors: {}",
                summary.files_with_tree_sitter_errors
            );
        }
        if summary.files_using_tier0_fallback > 0 {
            println!(
                "    Tier 0 fallback:    {}",
                summary.files_using_tier0_fallback
            );
        }
    }

    // Report skipped files if any.
    let skipped_total = summary.binary_skipped + summary.large_skipped + summary.error_skipped;
    if skipped_total > 0 {
        println!();
        println!("  Skipped files:");
        if summary.binary_skipped > 0 {
            println!("    Binary:     {}", summary.binary_skipped);
        }
        if summary.large_skipped > 0 {
            println!("    Too large:  {}", summary.large_skipped);
            if verbose {
                for (path, size) in &summary.large_skipped_paths {
                    println!("      - {path} ({size})", size = format_bytes(*size));
                }
            }
        }
        if summary.error_skipped > 0 {
            println!("    Read errors: {}", summary.error_skipped);
        }
        if !verbose && !summary.large_skipped_paths.is_empty() {
            println!();
            println!("  Rerun with --verbose (-v) to see skipped file paths.");
        }
    }

    // Report parse errors if any.
    if !summary.parse_errors.is_empty() {
        println!();
        println!("  Parse errors ({}):", summary.parse_errors.len());
        for err in &summary.parse_errors {
            println!("    {}: {}", err.file_path, err.error);
        }
    }

    // Special message for empty repos.
    if summary.files_parsed == 0 && summary.chunks_created == 0 {
        println!();
        println!("  No source files found to index.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vera_core::types::Language;

    /// Two results whose combined content far exceeds a small budget.
    fn two_results() -> Vec<vera_core::types::SearchResult> {
        let content = "fn a() {\n    body\n}\n".repeat(20);
        vec![
            vera_core::types::SearchResult {
                file_path: "src/a.rs".to_string(),
                line_start: 1,
                line_end: 3,
                content: content.clone(),
                language: Language::Rust,
                score: 0.9,
                symbol_name: Some("a".to_string()),
                symbol_type: None,
                part_index: None,
            },
            vera_core::types::SearchResult {
                file_path: "src/b.rs".to_string(),
                line_start: 10,
                line_end: 12,
                content,
                language: Language::Rust,
                score: 0.5,
                symbol_name: None,
                symbol_type: None,
                part_index: None,
            },
        ]
    }

    #[test]
    fn json_output_stays_parseable_within_the_character_budget() {
        let results = two_results();
        let contents: Vec<&str> = results.iter().map(|r| r.content.as_str()).collect();
        let full = json_results_string(&results, &contents);
        assert!(full.len() > 300, "fixture must exceed the budget below");

        let budgeted = json_within_budget(&results, &contents, 300);
        let parsed: serde_json::Value =
            serde_json::from_str(&budgeted).expect("budgeted JSON must parse");
        assert!(parsed.as_array().is_some_and(|a| !a.is_empty()));
        assert!(
            budgeted.len() < full.len(),
            "budget must drop or shorten results"
        );

        // Without a budget the document is untouched.
        assert_eq!(json_within_budget(&results, &contents, 0), full);
    }

    #[test]
    fn json_output_shortens_a_single_oversized_result() {
        let results = two_results();
        let contents: Vec<&str> = results.iter().map(|r| r.content.as_str()).collect();
        let budgeted = json_within_budget(&results[..1], &contents[..1], 120);
        assert!(budgeted.len() <= 120, "{} bytes", budgeted.len());
        let parsed: serde_json::Value =
            serde_json::from_str(&budgeted).expect("budgeted JSON must parse");
        assert_eq!(parsed.as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn json_budget_counts_escaped_content() {
        let mut results = two_results();
        results.truncate(1);
        results[0].content = "\"quoted\"\n".repeat(120);
        let contents = [results[0].content.as_str()];
        let budgeted = json_within_budget(&results, &contents, 120);
        assert!(budgeted.len() <= 120, "{} bytes", budgeted.len());
        let parsed: serde_json::Value =
            serde_json::from_str(&budgeted).expect("budgeted JSON must parse");
        assert_eq!(parsed.as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn json_budget_smaller_than_metadata_returns_empty_array() {
        let results = two_results();
        let contents: Vec<&str> = results.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(json_within_budget(&results, &contents, 10), "[]");
    }

    #[test]
    fn raw_output_spends_the_budget_across_results_and_drops_lower_ranked_ones() {
        // Single-line contents: truncation cannot stop early at a line
        // boundary, so each result spends its whole remaining allowance.
        let results = two_results();
        let results: Vec<vera_core::types::SearchResult> = results
            .into_iter()
            .map(|mut r| {
                r.content = "x".repeat(400);
                r
            })
            .collect();
        let contents: Vec<&str> = results.iter().map(|r| r.content.as_str()).collect();

        // No budget: both blocks print in full.
        let unlimited = format_raw_results(&results, &contents, 0);
        assert_eq!(unlimited.matches("(lines ").count(), 2);
        assert!(unlimited.contains("src/b.rs"));

        // A budget the first result exhausts exactly: its header stays
        // visible, but the second result is dropped entirely.
        let first_only = format_raw_results(&results, &contents, 200);
        assert!(first_only.contains("src/a.rs"));
        assert!(!first_only.contains("src/b.rs"), "{first_only}");
    }

    #[test]
    fn raw_output_format_is_stable_without_a_budget() {
        let mut result = two_results().remove(0);
        result.content = "line one\nline two\nline three\nline four\n".to_string();
        let out = format_raw_results(std::slice::from_ref(&result), &[result.content.as_str()], 0);
        assert_eq!(
            out,
            "1. src/a.rs (lines 1-3, rust)\n   \
             a\n   score: 0.900000\n   \
             │ line one\n   │ line two\n   │ line three\n\n"
        );
    }
}
