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
    full_text_only: bool,
    save: Option<std::path::PathBuf>,
    yes: bool,
) -> anyhow::Result<()> {
    let summary = if full_text_only {
        execute_full_text_only(
            path,
            exclude,
            no_ignore,
            no_default_excludes,
            no_progress,
            json_output,
            save.as_deref(),
            yes,
        )?
    } else {
        execute_with_confirmation(
            path,
            json_output,
            backend,
            exclude,
            no_ignore,
            no_default_excludes,
            no_progress,
            low_vram,
            save.as_deref(),
            yes,
        )?
    };

    if json_output {
        let json = serde_json::to_string_pretty(&summary)
            .map_err(|e| anyhow::anyhow!("failed to serialize summary: {e}"))?;
        println!("{json}");
    } else {
        print_human_summary(&summary, verbose);
    }

    fn execute_full_text_only(
        path: &str,
        exclude: Vec<String>,
        no_ignore: bool,
        no_default_excludes: bool,
        no_progress: bool,
        json_output: bool,
        save: Option<&Path>,
        yes: bool,
    ) -> anyhow::Result<vera_core::indexing::IndexSummary> {
        let repo_path = Path::new(path);
        if !repo_path.exists() {
            bail!("path does not exist: {path}\nHint: check the path and try again.");
        }
        if !repo_path.is_dir() {
            bail!(
                "path is not a directory: {path}\nHint: vera index expects a directory path, not a file."
            );
        }
        let replacement_confirmed =
            ensure_replacement_confirmed(repo_path, save, json_output, yes)?;

        let mut config = state::load_runtime_config()?;
        config.indexing.extra_excludes.extend(exclude);
        config.indexing.no_ignore |= no_ignore;
        config.indexing.no_default_excludes |= no_default_excludes;
        let rt = tokio::runtime::Runtime::new()
            .map_err(|error| anyhow::anyhow!("failed to create async runtime: {error}"))?;

        let show_progress = !no_progress && std::io::stderr().is_terminal();
        let multi = show_progress.then(|| cliclack::multi_progress("Indexing full text..."));
        let scan_spinner = Arc::new(std::sync::Mutex::new(
            multi
                .as_ref()
                .map(|multi| Arc::new(multi.add(cliclack::spinner()))),
        ));
        if let Some(spinner) = scan_spinner.lock().unwrap().as_ref() {
            spinner.start("Preparing full-text index...");
        }
        let indexing_bar = Arc::new(std::sync::Mutex::new(None));
        let scan_spinner_ref = Arc::clone(&scan_spinner);
        let indexing_bar_ref = Arc::clone(&indexing_bar);
        let multi_ref = multi.clone();
        let result = rt.block_on(
            vera_core::indexing::pipeline::index_repository_full_text_only_with_progress(
                repo_path,
                &config,
                &vera_core::CancellationToken::new(),
                replacement_confirmed,
                save,
                move |event| match event {
                    vera_core::indexing::pipeline::FullTextIndexProgress::DiscoveryStarted => {
                        if let Some(spinner) = scan_spinner_ref.lock().unwrap().as_ref() {
                            spinner.set_message("Scanning files...");
                        }
                    }
                    vera_core::indexing::pipeline::FullTextIndexProgress::DiscoveryDone {
                        file_count,
                    } => {
                        if let Some(spinner) = scan_spinner_ref.lock().unwrap().take() {
                            spinner.stop(format!("Scanned {file_count} files"));
                        }
                        if file_count > 0 {
                            if let Some(multi) = &multi_ref {
                                let bar = Arc::new(multi.add(cliclack::progress_bar(file_count as u64)));
                                bar.start(format!("Indexing files (0/{file_count})..."));
                                *indexing_bar_ref.lock().unwrap() = Some(bar);
                            }
                        }
                    }
                    vera_core::indexing::pipeline::FullTextIndexProgress::FilesIndexed {
                        files_done,
                        file_count,
                        chunks_created,
                    } => {
                        if let Some(bar) = indexing_bar_ref.lock().unwrap().as_ref() {
                            bar.set_position(files_done as u64);
                            bar.set_message(format!(
                                "Indexing files ({files_done}/{file_count}; {chunks_created} chunks)"
                            ));
                        }
                    }
                },
            ),
        );
        match result {
            Ok(summary) => {
                if let Some(bar) = indexing_bar.lock().unwrap().take() {
                    bar.stop(format!(
                        "Indexed {} files into {} full-text chunks",
                        summary.files_parsed, summary.chunks_created
                    ));
                }
                if let Some(spinner) = scan_spinner.lock().unwrap().take() {
                    spinner.stop(format!(
                        "Indexed {} files into {} full-text chunks",
                        summary.files_parsed, summary.chunks_created
                    ));
                }
                Ok(summary)
            }
            Err(error) => {
                let message = format!("Full-text indexing failed: {error}");
                if let Some(bar) = indexing_bar.lock().unwrap().take() {
                    bar.error(message);
                } else if let Some(spinner) = scan_spinner.lock().unwrap().take() {
                    spinner.error(message);
                }
                Err(error).context("full-text indexing failed")
            }
        }
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
    yes: bool,
) -> anyhow::Result<vera_core::indexing::IndexSummary> {
    execute_with_confirmation(
        path,
        json_output,
        backend,
        exclude,
        no_ignore,
        no_default_excludes,
        no_progress,
        low_vram,
        None,
        yes,
    )
}

fn execute_with_confirmation(
    path: &str,
    json_output: bool,
    backend: InferenceBackend,
    exclude: Vec<String>,
    no_ignore: bool,
    no_default_excludes: bool,
    no_progress: bool,
    low_vram: bool,
    save: Option<&Path>,
    yes: bool,
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
    let replacement_confirmed = ensure_replacement_confirmed(repo_path, save, json_output, yes)?;

    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| anyhow::anyhow!("failed to create async runtime: {e}"))?;

    let mut config = state::load_runtime_config()?;
    if low_vram {
        config.embedding.low_vram = true;
    }
    config.adjust_for_backend(backend);
    config.indexing.extra_excludes.extend(exclude);
    config.indexing.no_ignore |= no_ignore;
    config.indexing.no_default_excludes |= no_default_excludes;

    let (provider, model_name) = rt.block_on(vera_core::embedding::create_dynamic_provider(
        &config, backend,
    ))?;
    let provider = Arc::new(provider);
    let reporter = EmbeddingReporter::new(provider.clone(), !no_progress);
    let output_index_dir = save.map(Path::to_path_buf);

    // Show the progress display only for interactive, non-JSON runs. Mirrors
    // the same decision in `vera update` so both commands behave identically
    // when piped, redirected, or run with --no-progress.
    let show_progress = !json_output && !no_progress && std::io::stderr().is_terminal();
    if !show_progress {
        let cancellation = vera_core::CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let task_repo_path = repo_path.to_path_buf();
        let task_index_dir = output_index_dir.clone();
        let signal = wait_for_interrupt(rt.handle())?;
        let progress_reporter = reporter.clone();
        let tracker =
            std::sync::Mutex::new(vera_core::indexing::progress::HonestProgressTracker::new());
        let task = rt.handle().spawn(async move {
            vera_core::indexing::pipeline::index_repository_with_progress_and_cancellation_with_options(
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
                replacement_confirmed,
                task_index_dir.as_deref(),
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
    let task_index_dir = output_index_dir;
    let signal = wait_for_interrupt(rt.handle())?;
    let task = rt.handle().spawn(async move {
        vera_core::indexing::pipeline::index_repository_with_progress_and_cancellation_with_options(
            &task_repo_path,
            provider.as_ref(),
            &config,
            &model_name,
            on_progress,
            &task_cancellation,
            replacement_confirmed,
            task_index_dir.as_deref(),
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

fn ensure_replacement_confirmed(
    repo_path: &Path,
    save: Option<&Path>,
    json_output: bool,
    yes: bool,
) -> anyhow::Result<bool> {
    let canonical_repo = repo_path
        .canonicalize()
        .with_context(|| format!("failed to resolve path: {}", repo_path.display()))?;
    let index_dir = if let Some(path) = save {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        }
    } else {
        canonical_repo.join(".vera")
    };
    if !index_dir.exists() {
        return Ok(yes);
    }
    if yes {
        return Ok(true);
    }

    if json_output || !crate::helpers::prompts_available() {
        bail!(
            "an index already exists at {}; replacing it requires confirmation. Re-run with `--yes` to replace it without prompting.",
            index_dir.display()
        );
    }

    if !cliclack::confirm(format!(
        "An index already exists at {}. Rebuild and replace it?",
        index_dir.display()
    ))
    .initial_value(false)
    .interact()?
    {
        bail!("index replacement cancelled; the existing index was not changed");
    }
    Ok(true)
}
