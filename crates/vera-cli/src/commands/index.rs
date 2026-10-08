//! `vera index <path>` — Index a codebase for search.

use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use vera_core::config::InferenceBackend;
use vera_core::indexing::IndexProgress;

use crate::helpers::{
    EmbeddingReporter, cancel_task_on_signal, finalize_progress_ui, handle_embedding_done,
    print_human_summary, render_embed_display, stop_embed_spinner_on_parsing_done,
    wait_for_interrupt,
};
use crate::state;

/// Run the `vera index <path>` command.
#[allow(
    clippy::too_many_arguments,
    reason = "Index commands retain their existing CLI dispatch interface and independent options"
)]
pub fn run(
    path: &str,
    json_output: bool,
    backend: InferenceBackend,
    exclude: Vec<String>,
    no_ignore: bool,
    no_default_excludes: bool,
    no_progress: bool,
    verbose: bool,
    low_vram: bool,
) -> anyhow::Result<()> {
    let summary = execute(
        path,
        json_output,
        backend,
        exclude,
        no_ignore,
        no_default_excludes,
        no_progress,
        low_vram,
    )?;

    if json_output {
        let json = serde_json::to_string_pretty(&summary)
            .map_err(|e| anyhow::anyhow!("failed to serialize summary: {e}"))?;
        println!("{json}");
    } else {
        print_human_summary(&summary, verbose);
    }

    Ok(())
}

/// Index a repository and return the resulting summary.
#[allow(
    clippy::too_many_arguments,
    reason = "Index commands retain their existing CLI dispatch interface and independent options"
)]
pub fn execute(
    path: &str,
    json_output: bool,
    backend: InferenceBackend,
    exclude: Vec<String>,
    no_ignore: bool,
    no_default_excludes: bool,
    no_progress: bool,
    low_vram: bool,
) -> anyhow::Result<vera_core::indexing::IndexSummary> {
    let repo_path = Path::new(path);

    if !repo_path.exists() {
        bail!(
            "path does not exist: {path}\n\
             Hint: check the path and try again."
        );
    }
    if !repo_path.is_dir() {
        bail!(
            "path is not a directory: {path}\n\
             Hint: vera index expects a directory path, not a file."
        );
    }

    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| anyhow::anyhow!("failed to create async runtime: {e}"))?;

    let mut config = state::load_runtime_config()?;
    if low_vram {
        config.embedding.low_vram = true;
    }
    config.adjust_for_backend(backend);
    config.indexing.extra_excludes = exclude;
    config.indexing.no_ignore = no_ignore;
    config.indexing.no_default_excludes = no_default_excludes;

    let (provider, model_name) = rt.block_on(vera_core::embedding::create_dynamic_provider(
        &config, backend,
    ))?;
    let provider = Arc::new(provider);
    let reporter = EmbeddingReporter::new(provider.clone(), !no_progress);

    // Show the progress display only for interactive, non-JSON runs. Mirrors
    // the same decision in `vera update` so both commands behave identically
    // when piped, redirected, or run with --no-progress.
    let show_progress = !json_output && !no_progress && std::io::stderr().is_terminal();
    if !show_progress {
        let cancellation = vera_core::CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let task_repo_path = repo_path.to_path_buf();
        let signal = wait_for_interrupt(rt.handle())?;
        let progress_reporter = reporter.clone();
        let tracker =
            std::sync::Mutex::new(vera_core::indexing::progress::HonestProgressTracker::new());
        let task = rt.handle().spawn(async move {
            vera_core::indexing::pipeline::index_repository_with_progress_and_cancellation(
                &task_repo_path,
                provider.as_ref(),
                &config,
                &model_name,
                move |event| {
                    let mut tracker = tracker.lock().unwrap();
                    let display = tracker.handle(&event);
                    progress_reporter.observe(display, tracker.fixed_total());
                },
                &task_cancellation,
            )
            .await
        });
        let result = rt.block_on(reporter.wait(
            cancel_task_on_signal(task, signal, cancellation, "indexing"),
            false,
            |_| {},
        ));
        reporter.print_failure(&result);
        return result.context("indexing failed");
    }

    let multi = cliclack::multi_progress("Indexing...");
    let parse_spinner = Arc::new(multi.add(cliclack::spinner()));
    parse_spinner.start("Discovering files...");

    // Honest denominator: while parsing is still open, the embedding indicator
    // is open-ended (count without total / spinner) because cliclack 0.5.6 has
    // no unset-length API. Once `ParsingDone` arrives we switch to a fixed
    // total. The pure state machine lives in `vera_core::indexing::progress`.
    let tracker = Arc::new(std::sync::Mutex::new(
        vera_core::indexing::progress::HonestProgressTracker::new(),
    ));
    let embed_spinner: Arc<std::sync::Mutex<Option<Arc<cliclack::ProgressBar>>>> =
        Arc::new(std::sync::Mutex::new(None));
    let embed_bar: Arc<std::sync::Mutex<Option<Arc<cliclack::ProgressBar>>>> =
        Arc::new(std::sync::Mutex::new(None));

    let parse_spinner_ref = Arc::clone(&parse_spinner);
    let tracker_ref = Arc::clone(&tracker);
    let embed_spinner_ref = Arc::clone(&embed_spinner);
    let embed_bar_ref = Arc::clone(&embed_bar);
    let multi_ref = multi.clone();
    let progress_reporter = reporter.clone();

    let on_progress = move |event: IndexProgress| {
        // Update the pure tracker and decide what to render.
        let display = {
            let mut guard = tracker_ref.lock().unwrap();
            let display = guard.handle(&event);
            progress_reporter.observe(display.clone(), guard.fixed_total());
            display
        };
        match event {
            IndexProgress::DiscoveryDone { file_count } => {
                parse_spinner_ref.stop(format!("Discovered {file_count} files"));
                parse_spinner_ref.start("Parsing files...");
            }
            IndexProgress::ParsingDone { chunk_count } => {
                parse_spinner_ref.stop(format!("Parsed into {chunk_count} chunks"));
                stop_embed_spinner_on_parsing_done(
                    &embed_spinner_ref,
                    format!("Parsed into {chunk_count} chunks — finalizing embeddings..."),
                );
            }
            IndexProgress::EmbeddingProgress { .. } => {
                render_embed_display(
                    display,
                    progress_reporter.message(false).unwrap_or_default(),
                    &embed_spinner_ref,
                    &embed_bar_ref,
                    &multi_ref,
                );
            }
            IndexProgress::EmbeddingDone { .. } => {
                handle_embedding_done(display, &embed_spinner_ref, &embed_bar_ref, &multi_ref);
            }
            IndexProgress::StorageDone => {}
        }
    };

    let cancellation = vera_core::CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task_repo_path = repo_path.to_path_buf();
    let signal = wait_for_interrupt(rt.handle())?;
    let task = rt.handle().spawn(async move {
        vera_core::indexing::pipeline::index_repository_with_progress_and_cancellation(
            &task_repo_path,
            provider.as_ref(),
            &config,
            &model_name,
            on_progress,
            &task_cancellation,
        )
        .await
    });
    let result = rt.block_on(reporter.wait(
        cancel_task_on_signal(task, signal, cancellation, "indexing"),
        true,
        |message| crate::helpers::refresh_embed_message(message, &embed_spinner, &embed_bar),
    ));
    finalize_progress_ui(&result, &parse_spinner, &embed_spinner, &embed_bar, &multi);
    reporter.print_failure(&result);

    result.context("indexing failed")
}
