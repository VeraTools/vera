# Benchmarks

Vera is measured at three levels: end-to-end coding agent context efficiency, agent tool-choice appropriateness, and standalone code retrieval on a fixed public benchmark. Every ranking change must clear +0.5% on the full 1,251-task retrieval suite before shipping, and experiments that miss that bar stay documented in [benchmark history](benchmarks-history.md).

## Evaluations at a glance

- **Agent context efficiency (Flask, 4 arms, 40 sessions):** On 10 cross-file tracing questions answered by `GLM-5.3` (`high` effort) in Factory Droid, all four arms scored 10/10 blind-judged. Vera with `qwen/qwen3-embedding-8b` + `qwen/qwen3-reranker-8b` cut total prompt tokens by **48%** (`10.91M` vs `21.00M` control; paired `t = -3.6`, Wilcoxon `p = 0.037`, bootstrap 95% CI `-61.8%` to `-12.9%`), the only statistically significant reduction at `N = 10`. Local Vera (`15.39M`, `-27%`) observed more than twice Semble CLI's relative reduction (`18.49M`, `-12%`), though neither local arm's confidence interval excluded zero.
- **Skill tool-routing check (Flask, 5 arms, 8 probes x 3 reps):** Trigger- and restraint-focused skill instructions raised tool-choice appropriateness from 38% (bare) and 50% (older skill) to 75%, including 100% restraint on non-search probes (`git log`, `LICENSE`), while lowering mean tokens per probe from `21.5k` to `16.5k`.
- **Retrieval benchmark (historical snapshot, `Vera 2.0.1` vs `Semble 0.5.5`):** Using MinishLab's `potion-code-16M-v2` model on Semble v0.5.5's 1,251-task suite (63 repositories), Vera 2.0.1 scored `0.8436` nDCG@10 against Semble 0.5.5's `0.8514`, led slightly on Recall@5 (`0.9199` vs `0.9177`), and used `4.7 GB` of on-disk index space vs `32 GB`. On a 180-task independent check across 10 external repositories, Vera scored `0.7674` vs `0.7655`. On the 320-task slice, switching Vera to Qwen3 embeddings + reranking raised nDCG@10 from `0.8538` to `0.8647`.

## Agent context-efficiency benchmark

Retrieval scores test whether the right symbol ranks near the top. The agent benchmark tests the downstream goal: whether a coding agent spends less context exploring a repository when Vera is available.

### Setup and fairness controls

Ten cross-file tracing questions about Flask were run through fresh `droid exec` sessions (`custom:GLM-5.3-Free-[TokenRouter]-0` at `high` reasoning effort, 40 sessions total, ~`65.8M` cumulative prompt tokens and ~`29.8` hours of wall time). Every session ran with `FACTORY_HOME_OVERRIDE` pointed at an isolated config directory and `--disable-builtin-skills` so personal skills, global `AGENTS.md` files, and custom subagents could not leak across arms. Starting arm order rotated by question number, and `judge.py` graded all 40 answers blind against a verified answer key (`0` to `10` rubric).

- `control`: `vera` and `semble` shimmed to exit `127`, leaving `rg`/grep and file reads.
- `with-semble`: pre-warmed Semble index cache plus a root `AGENTS.md` documenting `semble search` and `semble find-related` CLI usage (`vera` shimmed out).
- `with-vera`: local Potion Code index (`VERA_LOCAL=1`), the shipped Droid skill, and the verbatim `AGENTS.md` routing block from `vera agent install` (`semble` shimmed out).
- `with-vera-qwen`: identical to `with-vera` except indexed and queried with the Qwen API preset (`qwen/qwen3-embedding-8b` + `qwen/qwen3-reranker-8b`).

### Results

| Arm | Blind score | Tool calls | Prompt tokens (input + cache read) | Search tool activations | Wall time |
|---|---:|---:|---:|---:|---:|
| Control (`rg`/grep + file reads) | 10.0/10 | 598 | 21.00M (baseline) | 0 | 7.7 h |
| Semble CLI (`potion-code-16M-v2`) | 10.0/10 | 566 | 18.49M (-12%) | 5 calls across 5/10 questions | 8.5 h |
| Vera, local Potion Code | 10.0/10 | 521 | 15.39M (-27%) | 10 calls across 5/10 questions | 6.9 h |
| **Vera, Qwen3 API (embedding + reranker)** | **10.0/10** | **453** | **10.91M (-48%)** | **16 calls across 8/10 questions** | **6.7 h** |

How to read this run:

- **Equal accuracy, different exploration cost.** At `high` reasoning effort, `GLM-5.3` solved all 10 questions even with grep alone, so every arm scored `10.0/10`. The difference shows up in how many files and tool turns the agent needed before it had enough context to answer.
- **Statistical significance.** Across the 10 paired questions, only `with-vera-qwen`'s 48% reduction is statistically significant (paired `t = -3.6`, Wilcoxon signed-rank `p = 0.037`, bootstrap 95% CI `-61.8%` to `-12.9%`). Local Vera's `-27%` reduction is more than double Semble's `-12%` in this sample, but both local arms' confidence intervals span zero at `N = 10` (neither differs significantly from control or from each other).
- **Selective activation.** Agents chose when to invoke search: each activated session's first call to Vera or Semble happened at or before its first exploratory search, and no session that skipped the tool on a path-following question picked it up mid-turn.

### Scope and earlier runs

This experiment covers 1 repository, 10 questions, 1 run per cell, and 1 agent model. Two earlier two-arm runs (`claude-opus-5` and `kimi-k3` at medium effort, 2026-08-16) tested an older binary and skill-only integration (before the `AGENTS.md` routing block and `FACTORY_HOME_OVERRIDE` isolation existed) and did not show the same savings direction (`+58%` and `+15%` total prompt tokens vs control, driven by cache-heavy follow-up reads on a few questions). While building the four-arm harness, auditing transcripts also caught setup bugs that required full re-runs (such as a fresh `VERA_HOME` leaving the Qwen reranker disabled, missing `AGENTS.md` parity, and rotated auth tokens). Per-question artifacts and reproduction scripts live in [`benchmarks/agent-bench/`](../benchmarks/agent-bench/README.md) and [benchmark history](benchmarks-history.md#agent-level-benchmark).

## Skill tool-routing evaluation

Context efficiency depends not just on search ranking, but on whether the agent invokes Vera for conceptual and structural questions while leaving exact string matches and non-code questions alone. [`benchmarks/skill-eval/`](../benchmarks/skill-eval/README.md) tests five integration configurations on Flask across 8 short read-only probes (expected tool class: `VERA`, `EXACT` via `rg`/`vera grep`, or `NONE` for `git log` and `LICENSE` restraint checks; `muse-spark-1.2-contributor` at `xhigh`, 3 repetitions per cell):

| Arm | `AGENTS.md` snippet | Skill installed | Tool-choice appropriateness | Mean tokens / probe |
|---|---|---|---:|---:|
| `bare` | no | no | 38% | 21.5k |
| `snippet` | yes | no | 50% | 22.0k |
| `skill` (earlier wording) | no | yes | 50% | 20.3k |
| `both` (earlier wording) | yes | yes | 50% | 18.2k |
| **`skill-v2` (trigger + restraint wording)** | **no** | **yes** | **75%** | **16.5k** |

`skill-v2` reached 100% on the structural lookup probe (where earlier variants scored 0 to 33%) and 100% on both restraint probes, while conceptual probes remained the hardest (`33%` to `67%`). Limits: 1 model, 1 repository, 8 probes, 3 repetitions, and rule-based command matching rather than answer grading.

## Retrieval benchmarks

### Semble benchmark, full suite (historical versioned comparison)

Vera's main retrieval suite and default local model come from [MinishLab](https://github.com/MinishLab), who publish both [`potion-code-16M-v2`](https://huggingface.co/minishlab/potion-code-16M-v2) and [Semble](https://github.com/MinishLab/semble). The benchmark from [Semble v0.5.5](https://github.com/MinishLab/semble/tree/v0.5.5) (commit `9218491`) contains 1,251 natural-language, cross-file, and symbol queries across 63 pinned open-source repositories in 19 languages.

Both tools ran `potion-code-16M-v2` on the same 1,251 tasks and were scored with the same graded nDCG@10 formula (see [Scoring](#scoring) for one matching difference). Because neither tool has been re-measured head-to-head against later Semble releases, treat this table as a controlled snapshot at `Vera 2.0.1` and `Semble 0.5.5` rather than a live claim about newer versions of either project.

| Tool (version snapshot) | nDCG@10 | Recall@1 | Recall@5 | Recall@10 | MRR | Query p50 | Index time | Index size |
|------|--------:|---------:|---------:|----------:|----:|----------:|-----------:|-----------:|
| Vera 2.0.1 | 0.8436 | 0.6711 | **0.9199** | 0.9502 | 0.8258 | 6.3 ms | 89 s | **4.7 GB** |
| Semble 0.5.5 | **0.8514** | **0.6747** | 0.9177 | **0.9656** | **0.8348** | 2.3 ms | 100 s | 32 GB |

How to read it:

- **Quality.** The two tools sit within 1% on nDCG@10 at those versions. Vera places a relevant answer in the top five slightly more often; Semble holds a small edge at rank 1 and rank 10.
- **Index footprint.** Vera's on-disk indexes across all 63 repositories total `4.7 GB` versus `32 GB` for Semble.
- **Query architecture.** Both answer in single-digit milliseconds at the median inside the benchmark runner. Semble searches vectors resident in memory inside a long-lived process; Vera memory-maps its `.vera/` files on disk so a fresh CLI invocation does not have to load vectors into RAM first (Vera p95 query latency: `59 ms`).
- **Timing caveat.** The Vera 2.0.1 row is the median of three full runs on 2026-10-09 on an AMD Ryzen 7 9800X3D (local `potion-code-16M-v2`, no reranker, indexes rebuilt each run). The Semble 0.5.5 row comes from the 2026-08-23 comparison and was not re-timed on that CPU, so the timing columns are reference points rather than a same-host race.

### Subset and independent contamination check

| Task set | Vera 2.0.1 | Semble 0.5.5 |
|---|---:|---:|
| 320-task tuning subset (16 Semble repositories) | **0.8538** | 0.8494 |
| 180-task independent set (10 external repositories) | **0.7674** | 0.7655 |

Scores are nDCG@10. Because MinishLab published both `potion-code-16M-v2` and the 63-repository Semble suite, the independent set tests generalization on 10 pinned repositories outside Semble's corpus (`celery`, `jinja`, `rayon`, `viper`, `lodash`, `ky`, `sidekiq`, `okhttp`, `zlib`, `nlohmann/json`) using 180 tasks written in the same three categories with verified answers. Both tools stay level on it, showing no sign that either the model or Vera's ranking rules depend on Semble-specific corpus quirks.

### Qwen3 API backend on the 320-task subset

With `qwen/qwen3-embedding-8b` embeddings and `qwen/qwen3-reranker-8b` reranking through OpenRouter, Vera scored **`0.8647` nDCG@10** on the 320-task subset (versus `0.8538` for local Potion Code) and gained three percentage points on Recall@1. A follow-up [reranker budget sweep](benchmarks-history.md#reranker-budget-sweep-2026-11-03) at v2.3.0 reduced the default rerank pool from 50 x 4,800 chars to 20 x 2,000 chars, cutting reranker input tokens by 68% within `0.001` nDCG@10. The Qwen pair was measured on the 320-task subset rather than the full 1,251-task suite; see [models](models.md) for backend setup.

## Methodology

### Provenance

The task set is Semble v0.5.5 at commit `9218491` (`921849164e2632dd4f0e1c1370f82cfe15ed6d6c`). The converter and corpus manifests pin that commit and record a content hash of every task file so accidental edits fail verification. The 320-task tuning subset is an unmodified 16-repository slice of that release.

### Scoring

Vera's harness uses graded relevance: primary answer locations have weight 2, secondary locations weight 1, and multiple chunks hitting the same ground-truth location count once. Scores are averaged across tasks. Semble's own published README tables use binary relevance averaged per repository and language, so they cannot be compared directly with the numbers here. Above, both tools are scored with Vera's graded formula, with one matching difference: a Vera hit counts only when its file matches and its line range overlaps the ground-truth span, whereas a Semble hit counts on file match. Across the full suite, 1,457 of 1,491 ground-truth locations span a whole file, so the two rules differ on 34 locations.

### What the numbers do and do not show

- Queries against an existing index are deterministic. Rebuilding an index from scratch can reorder tied chunks, moving full-suite nDCG@10 by roughly `0.0004`; smaller differences are tie-breaking noise.
- Ranking or chunking changes must improve full-suite nDCG@10 by at least `0.5%` without disproportionate latency or storage cost. Tested hypotheses that fell short (multiplicative path penalties at `-0.25%`, uniform 5x candidate pools at `+0.34%`, 750-char chunks at `-0.23%` with `+39%` storage, and structural graph pool augmentation at `+2.0 s` mean latency) were rejected and removed; see [benchmark history](benchmarks-history.md#rejected-ranking-and-chunking-hypotheses-2026-09-01) and [architecture decisions](adr/000-decision-summary.md).
- Benchmark figures compare configurations on fixed corpora. Your repository structure, language mix, and hardware will shift absolute latencies and token counts.

### Reproduce

```bash
bash eval/setup-semble-corpus.sh          # clone the 63 pinned repositories
cargo build --locked --release
cargo run --locked --release --bin vera-eval -- run \
  --tool vera-potion \
  --tasks-dir eval/tasks/semble \
  --corpus eval/semble-corpus.toml \
  --json-only
```

## Earlier comparisons

Before adopting Semble's 1,251-task benchmark, pre-1.0 versions of Vera were tested on smaller internal suites with different models. Keep them as historical context rather than current head-to-head claims:

- On a 21-task suite across `ripgrep`, `flask`, `fastify`, and `turborepo`, Vera 0.7.0 scored `0.836` nDCG@10 against `0.561` for ColGREP.
- On a 17-task suite across `ripgrep`, `flask`, and `fastify`, Vera hybrid search scored `0.801` nDCG@10 against `0.521` for `cocoindex-code`, `0.708` for vector-only search, and `0.293` for `ripgrep`.

Full historical tables are in [benchmark history](benchmarks-history.md).
