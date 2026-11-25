# Deshcmd Phase 4 performance results

## Directory snapshots

The cache retains completed ordinary-directory snapshots keyed by native directory path. Explicit navigation uses resolved paths, while startup and restored tabs use their supplied paths. Recursive, flat, archive and virtual views keep their own scan scopes. One scan can serve both active panes. A pane can detach without invalidating the other subscriber. Raw rows and cached index buffers are shared, and the tab owns its own sort, filter, marks, cursor and history.

The LRU retains at most 32 directories and an estimated 64 MiB. It charges entry capacity, owned native paths and strings, the raw path index, and one compatible prepared view per directory. Shared raw rows and cached index arrays count once. Oversized snapshots remain usable in the active pane. Inactive tabs release view and raw references while retaining path-based selection and focus. The budget covers cache retention. Active snapshots, transient scan and patch buffers, allocator retention and pending retirements also contribute to process RSS.

A compatible ready view can appear as soon as navigation validates. History can show a short, read-only preview above the committed file list during validation. The committed rows remain visible and actionable. Failure or supersession withdraws the preview. A cache entry validated within four seconds can satisfy navigation while its watch coverage continues. A coverage gap, invalidation, or expiry schedules a replacement scan while the cached rows stay visible. The cache uses validation age and explicit invalidation rather than directory modification time.

## Filesystem changes

- **Jobs.** The job worker derives a bounded set of potentially affected items, parent directories and subtrees. Completion applies these effects even after partial failure or cancellation. Directory effects match both the requested and resolved paths. Multi-volume archive packing also invalidates virtual-list members in the output directory. Read-only operations skip invalidation. Large or ambiguous effect sets fall back to invalidating all locations. Explicit Refresh retains its full validation behavior.
- **Watching.** Two managed slots own native nonrecursive `notify` subscriptions. The same path uses one slot across panes. Registration and teardown run off the frame thread. A scan worker waits up to 50 ms for registration, and late or restored coverage requests reconciliation. Callbacks coalesce hints over 40 ms of quiet time, bounded by 200 ms, and fence events by subscription revision. Overflow, watch failure and watched-directory replacement request a full scan. Active ordinary listings with coverage reconcile every 60 seconds for missed events. Uncovered directory, recursive and virtual-list views use four-second completion-relative fallback. Flat and archive views retain manual and job-driven refresh.
- **Patches.** A small batch of direct child changes re-reads those paths and a bounded set of existing symlinks. It checks directory identity before and after the read and commits only against the same raw snapshot and watch observation. Unix snapshots store device and inode values without retaining a filesystem descriptor. Other platforms use full reconciliation until a descriptor-free identity and link count are available. View preparation still runs on a worker and reconciles current marks and cursor by native path. Incremental updates fall back to complete reconciliation for potentially affected hard-link aliases, renames, `descript.ion`, skipped rows, unclear events, a superseded base or a batch exceeding the initial 32-path limit. The threshold is a conservative starting point. Patches still copy the immutable row data and rebuild indices in O(N) work.

## Verification and limits

The native Linux/ext4 tests cover a real external write, idle watch coverage, shared subscriptions, and change convergence. Injected hints and gates cover duplicate delivery, events during enumeration, backend errors, replacement at the same path, overflow of the bounded path batch, and a manual refresh superseding a queued patch. Other tests cover history failure without operand changes, cache eviction with a live reference, independent pane marks and sort order, read-only and partial jobs, rename and move invalidation, comment sidecars, reconciliation against an independent scan, and fallback refresh of virtual lists. The review regressions cover a watch coverage gap, descriptor release, hard-link sizes and marked totals, job invalidation through a symlink, generated archive volumes, and visibility of committed action targets during history preview. Formatting, 163 default workspace tests, 164 all-feature tests, and strict Clippy with both feature sets passed. The release benchmarks passed separately.

Native Windows, macOS and remote or network filesystem behavior remain unverified. A backend that registers successfully but omits notifications is covered by periodic reconciliation. External changes to out-of-directory symlink targets also rely on reconciliation. Unvalidated aliases in directly supplied startup or workspace paths can be scanned separately. The 64 MiB estimate excludes allocator overhead. Isolated OS-cold or device-cold timings and native window presentation latency were unavailable in this run.

## Release measurements

Phase 3 is commit `51aae98`. Both revisions used rustc 1.96.0 and the release profile on an Intel Core i5-1145G7 with Linux 6.8.0-149-generic and local ext4. Each process used the same generated fixture. File creation preceded measurement, so the OS cache was warm or uncontrolled. “First” means the application had no snapshot for that directory. Five repeated visits per size provide the revisit p50 and p95. The single creation, two modifications, one move and one burst per size are reported as individual observations or small-sample summaries.

The engine workloads were rerun after the Phase 4 review fixes. Revisits retain the first-row latency benefit and now wait for background revalidation at the complete-view milestone. The table uses the 100k mixed directory unless indicated. The simultaneous-pane case uses the separate plain 100k directory. Timings are engine-ready unless called out as headless egui frame time.

| Metric | Phase 3 | Phase 4 |
|---|---:|---:|
| Revisit to first usable rows, p50 / p95 | 1.214 / 4.222 ms | 0.031 / 0.104 ms |
| Revisit to complete view, p50 / p95 | 242.470 / 253.043 ms | 254.912 / 259.684 ms |
| Completed full scans across five revisits | 5 | 5 |
| Completed scans when both panes open plain 100k | 2 | 1 |
| Completed scans in an idle minute, one 100k pane and one empty pane | 28 | 0 |
| Idle row metadata attempts in that minute | 1,400,070 | 0 |
| Creation to visible entry | 970.635 ms | 150.883 ms |
| Single-file write to updated size, median of two | 3,996.871 ms | 165.091 ms |
| Entry metadata paths read per single-file write on the mixed fixture | 100,006 | 3 |
| External cross-directory move to visible in both panes | 4,005.226 ms | 299.945 ms |
| 10k-file burst to all rows visible | 4,033.878 ms | 468.859 ms |
| Full scans during that burst across both panes | 2 | 1 |
| Snapshot lookup hit rate across the 100k workload | — | 12/18 (67%) |
| Cache bytes retained at process end | — | 40,113,555 bytes (38.3 MiB) |
| Peak RSS for the entire 100k workload | 151,272 KiB | 191,768 KiB |
| p95 headless egui loading frame | 0.302 ms | 0.300 ms |
| p95 headless egui steady frame, all 100k marked | 0.207 ms | 0.198 ms |

The patched mixed listing makes three entry metadata attempts for a single-file write because it also reevaluates the fixture’s two symlinks. Directory identity checks, sidecar reads, symlink target calls and filesystem bookkeeping are outside that entry counter. The one-minute row counter measures listing metadata attempts. It does not count unrelated UI probes. The steady-frame p95 is based on 50 draws on each build, and the loading p95 uses 205 Phase 3 frames and 202 Phase 4 frames. Frame timing covers egui logic and draw submission. Native presentation remains unmeasured.

| Mixed fixture | First complete, Phase 3 → Phase 4 | Revisit complete p50 / p95, Phase 3 → Phase 4 | Create visible, Phase 3 → Phase 4 | Peak RSS, Phase 3 → Phase 4 |
|---|---:|---:|---:|---:|
| 100 entries | 0.491 → 0.639 ms | 0.314 / 0.340 → 0.372 / 0.407 ms | 2,718.827 → 42.281 ms | 87,936 → 51,316 KiB |
| 10,000 entries | 22.380 → 35.526 ms | 20.366 / 21.748 → 20.066 / 21.496 ms | 2,560.918 → 64.132 ms | 88,104 → 72,808 KiB |
| 100,000 entries | 263.387 → 292.234 ms | 242.470 / 253.043 → 254.912 / 259.684 ms | 970.635 → 150.883 ms | 151,272 → 191,768 KiB |

Each process opens the plain 100k directory in both panes, so even the small-fixture RSS rows include that workload. The 100k mixed and plain snapshots exceed the retention budget together. The 100k run ends with two retained cache entries, and the 100 and 10k runs end with three at 43,384,309 and 47,983,845 bytes respectively. The lookup hit rate in both smaller runs is 13/18 (72%). The release runs prioritize comparable work counts. The background revisit scan can increase peak RSS while the cached and replacement snapshots overlap. Sub-millisecond tails and single-run latency remain sensitive to host scheduling.

## Reproduce

Use the [fixture generator](performance-fixture.py) in a disposable directory. Copy `crates/deshcmd-engine/tests/cache_performance.rs` into a Phase 3 checkout to build the same workload on both revisions. Build each revision and keep its executable.

```bash
python3 docs/performance-fixture.py /tmp/deshcmd-phase4-fixture
cargo test --release -p deshcmd-engine --features perf --test cache_performance --no-run
cargo test --release -p deshcmd-ui --features perf --lib --no-run
```

Set `ENGINE_TEST` and `UI_TEST` to the release executables printed by Cargo. Record process RSS separately per run and use the same local fixture for both revisions.

```bash
export DESHCMD_PERF_ROOT=/tmp/deshcmd-phase4-fixture DESHCMD_PERF_STAGES=1
for size in 100 10000 100000; do
  /usr/bin/time -v -o "rss-$size.txt" env DESHCMD_PERF_SIZE="$size" \
    "$ENGINE_TEST" --ignored --nocapture phase4_cache_and_refresh > "engine-$size.log" 2>&1
done
"$ENGINE_TEST" --ignored --nocapture phase4_idle_scan_rate > idle.log 2>&1
"$UI_TEST" --ignored --nocapture performance::phase1_ui > ui.log 2>&1
```

Count `stage,scan` records between the workload markers. The `patch,metadata_paths` and `cache` lines report patch reads and cache counters. The detailed scan counts include complete enumerations only.
