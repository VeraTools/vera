//! `vera estimate <path>` — estimate indexing work from a sampled file set.

use std::io::IsTerminal;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use humanize_duration::{Truncate, prelude::DurationExt};
use serde::Serialize;
use vera_core::config::InferenceBackend;
use vera_core::embedding::create_dynamic_provider;
use vera_core::indexing::estimate::{IndexEstimate, sample_index_work};

#[derive(Debug, Serialize)]
struct EstimateOutput {
    estimate: IndexEstimate,
    benchmark_chunks: usize,
    benchmark_chunk_limit: usize,
    full_text_benchmark: Option<FullTextBenchmark>,
    benchmarks: Vec<BackendBenchmark>,
}

#[derive(Debug, Serialize)]
struct FullTextBenchmark {
    indexed_chunks: usize,
    benchmark_secs: f64,
    chunks_per_sec: f64,
    estimated_secs: Option<f64>,
}

#[derive(Debug, Serialize)]
struct BackendBenchmark {
    backend: String,
    model: String,
    embedded_chunks: usize,
    initialization_secs: f64,
    benchmark_secs: f64,
    chunks_per_sec: f64,
    estimated_embedding_secs: Option<f64>,
    estimated_full_index_secs: Option<f64>,
    estimated_total_secs: Option<f64>,
}

#[allow(
    clippy::too_many_arguments,
    reason = "Estimate command keeps its CLI arguments explicit at the command-dispatch boundary."
)]
pub fn run(
    path: &str,
    json_output: bool,
    sample_files: usize,
    benchmark_chunks: usize,
    seed: Option<u64>,
    benchmark_backends: Vec<InferenceBackend>,
    benchmark_full_text: bool,
    exclude: Vec<String>,
    no_ignore: bool,
    no_default_excludes: bool,
    low_vram: bool,
) -> anyhow::Result<()> {
    if benchmark_backends.is_empty() && !benchmark_full_text {
        bail!(
            "select at least one benchmark: --benchmark-backend <BACKEND> and/or \
             --benchmark-full-text. Supported backends: api, potion-code, onnx-jina-cpu, \
             onnx-jina-cuda, onnx-jina-rocm, onnx-jina-directml, onnx-jina-coreml, \
             onnx-jina-openvino. Run `vera estimate --help` for descriptions and examples."
        );
    }
    if benchmark_chunks == 0 {
        bail!("benchmark chunk limit must be greater than zero");
    }
    if benchmark_backends
        .iter()
        .enumerate()
        .any(|(index, backend)| benchmark_backends[..index].contains(backend))
    {
        bail!("each --benchmark-backend value must be unique");
    }

    let repo_path = Path::new(path);
    if !repo_path.exists() {
        bail!("path does not exist: {path}");
    }
    if !repo_path.is_dir() {
        bail!("path is not a directory: {path}");
    }

    let mut config = crate::state::load_runtime_config()?;
    config.indexing.extra_excludes.extend(exclude);
    config.indexing.no_ignore |= no_ignore;
    config.indexing.no_default_excludes |= no_default_excludes;
    if low_vram {
        config.embedding.low_vram = true;
    }

    let show_progress = !json_output && std::io::stderr().is_terminal();
    let multi = show_progress.then(|| cliclack::multi_progress("Estimating indexing work..."));
    let scan_spinner = multi.as_ref().map(|multi| multi.add(cliclack::spinner()));
    if let Some(spinner) = &scan_spinner {
        spinner.start("Discovering files and parsing the stratified sample...");
    }
    let sample = match sample_index_work(repo_path, &config, sample_files, seed) {
        Ok(sample) => sample,
        Err(error) => {
            if let Some(spinner) = &scan_spinner {
                spinner.stop(format!("Estimate failed: {error}"));
            }
            return Err(error);
        }
    };
    if let Some(spinner) = &scan_spinner {
        spinner.stop(format!(
            "Parsed file sample: {} files / {} -> {} chunks; indexable population: {} ({} files)",
            sample.estimate.sampled_files,
            format_bytes(sample.estimate.sampled_bytes),
            sample.estimate.sampled_chunks,
            format_bytes(sample.estimate.total_bytes),
            sample.estimate.total_files,
        ));
    }
    let mut benchmarks = Vec::with_capacity(benchmark_backends.len());
    let runtime = tokio::runtime::Runtime::new()
        .context("failed to create async runtime for provider benchmarks")?;
    let selected_chunks = sample
        .benchmark_chunks(benchmark_chunks)
        .into_iter()
        .collect::<Vec<_>>();
    let full_text_benchmark = if !benchmark_full_text || selected_chunks.is_empty() {
        None
    } else {
        let spinner = multi.as_ref().map(|multi| multi.add(cliclack::spinner()));
        if let Some(spinner) = &spinner {
            spinner.start(format!(
                "Benchmarking full-text indexing with {} sampled chunks...",
                selected_chunks.len()
            ));
        }
        let temp_dir = tempfile::tempdir().context("failed to create temporary full-text index")?;
        let bm25 = vera_core::storage::bm25::Bm25Index::open(&temp_dir.path().join("bm25"))
            .context("failed to initialize full-text benchmark index")?;
        let writer = bm25
            .begin_bulk_build()
            .context("failed to create full-text benchmark writer")?;
        let full_text_start = Instant::now();
        if let Err(error) = writer.insert_chunks(&selected_chunks) {
            if let Some(spinner) = &spinner {
                spinner.stop(format!("Full-text benchmark failed: {error}"));
            }
            return Err(error).context("failed to index full-text benchmark sample");
        }
        if let Err(error) = writer.finish() {
            if let Some(spinner) = &spinner {
                spinner.stop(format!("Full-text benchmark failed: {error}"));
            }
            return Err(error).context("failed to finish full-text benchmark index");
        }
        let benchmark_secs = full_text_start.elapsed().as_secs_f64();
        let chunks_per_sec = selected_chunks.len() as f64 / benchmark_secs;
        let estimated_secs =
            (chunks_per_sec > 0.0).then(|| sample.estimate.estimated_chunks / chunks_per_sec);
        if let Some(spinner) = &spinner {
            spinner.stop(format!(
                "Full-text sample indexed: {:.1} chunks/s",
                chunks_per_sec
            ));
        }
        Some(FullTextBenchmark {
            indexed_chunks: selected_chunks.len(),
            benchmark_secs,
            chunks_per_sec,
            estimated_secs,
        })
    };

    for backend in benchmark_backends {
        if selected_chunks.is_empty() {
            break;
        }
        let benchmark_spinner = multi.as_ref().map(|multi| multi.add(cliclack::spinner()));
        if let Some(spinner) = &benchmark_spinner {
            spinner.start(format!(
                "Initializing {backend} for {} benchmark chunks...",
                selected_chunks.len(),
            ));
        }
        let mut backend_config = config.clone();
        backend_config.adjust_for_backend(backend);
        let initialization_start = Instant::now();
        let (provider, model) = match runtime
            .block_on(create_dynamic_provider(&backend_config, backend))
        {
            Ok(provider) => provider,
            Err(error) => {
                if let Some(spinner) = &benchmark_spinner {
                    spinner.stop(format!("Failed to initialize {backend}: {error}"));
                }
                return Err(error)
                    .with_context(|| format!("failed to initialize benchmark provider {backend}"));
            }
        };
        let initialization_secs = initialization_start.elapsed().as_secs_f64();
        if let Some(spinner) = &benchmark_spinner {
            spinner.stop(format!("Initialized {backend}; starting benchmark"));
        }
        let benchmark_bar = multi
            .as_ref()
            .map(|multi| multi.add(cliclack::progress_bar(selected_chunks.len() as u64)));
        if let Some(bar) = &benchmark_bar {
            bar.start(format!(
                "Benchmarking {backend}: 0 / {} chunks from sampled files",
                selected_chunks.len()
            ));
        }
        let (batch_size, concurrency) = backend_config.embedding.bounded_parallelism();
        let benchmark_start = Instant::now();
        if let Err(error) =
            runtime.block_on(vera_core::embedding::embed_chunks_concurrent_with_progress(
                &provider,
                &selected_chunks,
                batch_size,
                concurrency,
                backend_config.indexing.max_chunk_bytes,
                |done, total| {
                    if let Some(bar) = &benchmark_bar {
                        bar.set_position(done as u64);
                        bar.set_message(format!(
                            "Benchmarking {backend}: {done} / {total} chunks from sampled files"
                        ));
                    }
                },
            ))
        {
            if let Some(bar) = &benchmark_bar {
                bar.error(format!("Embedding benchmark failed for {backend}: {error}"));
            }
            return Err(error).with_context(|| format!("embedding benchmark failed for {backend}"));
        }
        let benchmark_secs = benchmark_start.elapsed().as_secs_f64();
        let chunks_per_sec = if benchmark_secs > 0.0 {
            selected_chunks.len() as f64 / benchmark_secs
        } else {
            0.0
        };
        let estimated_embedding_secs =
            (chunks_per_sec > 0.0).then(|| sample.estimate.estimated_chunks / chunks_per_sec);
        let estimated_total_secs = estimated_embedding_secs
            .zip(
                full_text_benchmark
                    .as_ref()
                    .and_then(|benchmark| benchmark.estimated_secs),
            )
            .map(|(embedding_secs, full_text_secs)| {
                sample.estimate.discovery_secs
                    + sample.estimate.estimated_parse_secs
                    + initialization_secs
                    + full_text_secs
                    + embedding_secs
            });
        if let Some(bar) = &benchmark_bar {
            bar.stop(format!(
                "{backend}: embedded {} sample chunks ({:.1} chunks/s)",
                selected_chunks.len(),
                chunks_per_sec
            ));
        }

        benchmarks.push(BackendBenchmark {
            backend: backend.to_string(),
            model,
            embedded_chunks: selected_chunks.len(),
            initialization_secs,
            benchmark_secs,
            chunks_per_sec,
            estimated_embedding_secs,
            estimated_full_index_secs: full_text_benchmark
                .as_ref()
                .and_then(|benchmark| benchmark.estimated_secs),
            estimated_total_secs,
        });
    }

    let output = EstimateOutput {
        estimate: sample.estimate,
        benchmark_chunks: selected_chunks.len(),
        benchmark_chunk_limit: benchmark_chunks,
        full_text_benchmark,
        benchmarks,
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&output).context("failed to serialize estimate output")?
        );
    } else {
        print_human_output(&output, path);
    }
    Ok(())
}

fn print_human_output(output: &EstimateOutput, path: &str) {
    let estimate = &output.estimate;
    println!("Index estimate: {path}");
    println!();
    println!("  Eligible files:       N = {}", estimate.total_files);
    println!(
        "  Indexable source size: B = {}",
        format_bytes(estimate.total_bytes)
    );
    println!("  (after ignore, exclusion, binary, and file-size filters; not raw directory size)");
    println!(
        "  Sample:               n = {} files ({:.2}%)",
        estimate.sampled_files,
        percentage(estimate.sampled_files, estimate.total_files)
    );
    println!("  Chunks from file sample: c = {}", estimate.sampled_chunks);
    println!(
        "  Bytes in sampled files: {}",
        format_bytes(estimate.sampled_bytes)
    );
    println!(
        "  Benchmark subset:      {} of {} chunks from sampled files (cap {})",
        output.benchmark_chunks, estimate.sampled_chunks, output.benchmark_chunk_limit
    );
    println!("  Random seed:           {}", estimate.seed);
    println!();
    println!(
        "  Estimated chunks:     Ĉ ≈ {}",
        format_count(estimate.estimated_chunks)
    );
    let (lower, upper) = estimate.chunk_count_95_interval;
    println!(
        "  95% interval:         [{}, {}] ({})",
        format_count(lower),
        format_count(upper),
        estimate.chunk_count_interval_method
    );
    println!(
        "  Parse estimate:        ~{} (sample parse {}; discovery {})",
        format_duration(estimate.estimated_parse_secs),
        format_duration(estimate.sampled_parse_secs),
        format_duration(estimate.discovery_secs)
    );
    println!();
    println!("Estimated indexing time (point estimate):");
    if !output.benchmarks.is_empty() && output.full_text_benchmark.is_some() {
        for benchmark in &output.benchmarks {
            match benchmark.estimated_total_secs {
                Some(total_secs) => {
                    println!("  {}: ~{}", benchmark.backend, format_duration(total_secs))
                }
                None => println!(
                    "  {}: unavailable (benchmark produced no measurable throughput)",
                    benchmark.backend
                ),
            }
        }
    } else if let Some(full_text) = &output.full_text_benchmark {
        if let Some(full_text_secs) = full_text.estimated_secs {
            println!(
                "  Discovery + parsing + full-text index: ~{}",
                format_duration(
                    estimate.discovery_secs + estimate.estimated_parse_secs + full_text_secs
                )
            );
        } else {
            println!("  Full-text stage estimate unavailable.");
        }
        println!("  Embedding not benchmarked; this is not a complete indexing-time estimate.");
    } else if !output.benchmarks.is_empty() {
        println!("  Embedding stage estimates:");
        for benchmark in &output.benchmarks {
            match benchmark.estimated_embedding_secs {
                Some(seconds) => {
                    println!("    {}: ~{}", benchmark.backend, format_duration(seconds))
                }
                None => println!("    {}: unavailable", benchmark.backend),
            }
        }
        println!(
            "  Full-text indexing not benchmarked; this is not a complete indexing-time estimate."
        );
    } else {
        println!("  unavailable: no benchmark was selected");
    }
    if output.full_text_benchmark.is_some() && !output.benchmarks.is_empty() {
        println!(
            "  Includes discovery, estimated parsing, full-text indexing, provider initialization, and embedding."
        );
        println!("  Vector/metadata storage and final publication are not measured.");
    }
    println!();
    println!("Estimator: stratified chunks-per-byte ratio (language × log₂ file-size strata).");
    if !output.benchmarks.is_empty() {
        println!("Each provider uses the same benchmark subset.");
    }
    if output.benchmark_chunks == 0 {
        println!("No chunks were produced by the sample; provider benchmarks were skipped.");
    }

    for benchmark in &output.benchmarks {
        println!();
        println!(
            "Embedding benchmark — {} ({})",
            benchmark.model, benchmark.backend
        );
        println!("  Benchmark chunks:     {}", benchmark.embedded_chunks);
        println!(
            "  Provider init:        {}",
            format_duration(benchmark.initialization_secs)
        );
        println!(
            "  Benchmark time:       {}",
            format_duration(benchmark.benchmark_secs)
        );
        println!(
            "  Throughput:           {:.1} chunks/s",
            benchmark.chunks_per_sec
        );
        if let Some(full_text_secs) = benchmark.estimated_full_index_secs {
            println!(
                "  Estimated full-text index: {}",
                format_duration(full_text_secs)
            );
        }
        if let Some(embedding_secs) = benchmark.estimated_embedding_secs {
            println!(
                "  Estimated embedding:  {}",
                format_duration(embedding_secs)
            );
        }
        if let Some(total_secs) = benchmark.estimated_total_secs {
            println!("  Estimated total:      {}", format_duration(total_secs));
        }
        if let Some(full_text) = &output.full_text_benchmark {
            println!();
            println!("Full-text indexing benchmark — Tantivy");
            println!("  Benchmark chunks:     {}", full_text.indexed_chunks);
            println!(
                "  Benchmark time:       {}",
                format_duration(full_text.benchmark_secs)
            );
            println!(
                "  Throughput:           {:.1} chunks/s",
                full_text.chunks_per_sec
            );
            if let Some(estimated_secs) = full_text.estimated_secs {
                println!(
                    "  Estimated full index: {}",
                    format_duration(estimated_secs)
                );
            }
        }
    }
}

fn percentage(sampled: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        sampled as f64 * 100.0 / total as f64
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2} {}", UNITS[unit])
}

fn format_count(count: f64) -> String {
    if count >= 1_000_000.0 {
        format!("{:.2} M", count / 1_000_000.0)
    } else if count >= 1_000.0 {
        format!("{:.2} k", count / 1_000.0)
    } else {
        format!("{count:.0}")
    }
}

fn format_duration(seconds: f64) -> String {
    Duration::from_secs_f64(seconds.max(0.0))
        .human(Truncate::Second)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_estimate_units() {
        assert_eq!(format_count(1_500.0), "1.50 k");
        assert_eq!(format_bytes(1024), "1.00 KiB");
        assert_eq!(format_duration(90.0), "1m 30s");
        assert_eq!(format_duration(5.695), "5s");
        assert_eq!(format_duration(1_300.4), "21m 40s");
        assert_eq!(format_duration(3_661.0), "1h 1m 1s");
        assert_eq!(format_duration(172_800.0), "2days");
        assert_eq!(percentage(1, 4), 25.0);
    }
}
