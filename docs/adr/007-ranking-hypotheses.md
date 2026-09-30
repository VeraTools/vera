# ADR 007: Rejected Ranking and Chunking Experiments

Status: rejected after measurement; implementations and controls removed in v2.0.0.

## Decision

Remove the multiplicative path penalty, uniform candidate-pool multiplier, and character-cap chunking experiment. Their full-suite results did not meet the 0.5% aggregate quality bar. Retaining dormant runtime branches, configuration aliases, and environment overrides adds maintenance cost without a supported use case.

The accepted filename-stem boost, definition-content boost, and query-dependent recall-pool expansion remain unchanged. Their mechanisms are recorded in [ADR 006](006-ranking-signals.md). The rejected uniform multiplier was a separate signal from that recall expansion.

## Mechanisms Tested

- A 0.3× path penalty tried to demote keyword-dense test, compatibility, and example content proportionally to retrieval confidence. Explicit requests for those categories bypassed the penalty. It added no full-suite value beyond the existing additive priors.
- A uniform 5× candidate-pool multiplier tried to expose more low-ranked candidates to ranking on every query. Its gain was below the acceptance bar; the supported pool expansion remains query-dependent.
- A 750-character cap tried to isolate concepts inside long symbols while preserving line boundaries and split-symbol identities. It fragmented context and increased indexing, storage, and query costs while regressing the full suite.

Parameters came from mechanism hypotheses, not benchmark answer inspection or heuristic retuning. The experiments were evaluated with and without each signal on the subset, independent set, and full suite. [Benchmark history](../benchmarks-history.md#rejected-ranking-and-chunking-hypotheses-2026-09-01) preserves the measurements, source commit, model, and decision.

Structural graph augmentation was also removed. Its bounded caller and implementation expansion gained too little quality for its reranking latency cost; see the [historical full-pipeline ablations](../benchmarks-history.md#ablations). Reference lookup and its receiver-disambiguation evaluation remain supported.

## Compatibility

Legacy configuration files still load through unknown-field tolerance. Removed settings disappear from configuration output and setters, and their environment variables have no effect.

Ordinary indexes keep their format and remain reusable. Any nonzero historical character-cap metadata, including either old alias or conflicting aliases, requires a full rebuild before search or incremental update. This check reads raw metadata so deserialization cannot discard the incompatibility. See [v2 migration](../migration-v2.md).
