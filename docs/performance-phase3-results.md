# Deshcmd Phase 3 performance results

## Architecture

A scan publishes immutable, reference-counted entries in their original enumeration order. A bounded two-worker pool prepares a separate order and projection for the visible tabs. The result carries the tab ID, scan generation, raw publication revision, and view revision. A newer navigation, raw publication, sort, or filter can supersede the request. Sorting checks for cancellation between runs and while merging them. Filtering reuses the previous order when the raw data and sort are unchanged. Sort and visibility preferences also invalidate inactive views, which are prepared when activated.

Rows, operands, marks, and focus retain their native paths. The worker builds a path index for each raw snapshot and reconciles a reference-counted selection if the visible membership or raw data changed. An edit during preparation is picked up before commit. A small changed selection can reconcile against the ready index at install time. A larger changed selection is reconciled on a worker using the same prepared view. Large row buffers use a four-item retirement queue. If it fills, the caller releases the buffer directly.

A fresh ordinary scan publishes at most one provisional page. It collects up to 64 eligible, fully materialized entries and sends the sample once that bound or the 16 ms check between entries is reached. The page mailbox holds at most one sample per active scan. Enumeration continues and publishes a separate complete result. Recursive, flat, virtual-list and archive scans publish their full model. Scope changes with retained marks also wait for the complete model so mark reconciliation can see all eligible entries. Refresh retains the previous snapshot and can apply sort and filter edits to it while the replacement scan runs.

Provisional rows show a loading count. A known item can be selected, previewed, opened or used as a file-operation operand. Whole-directory commands wait for a complete view with the requested filter membership. Pointer identity includes the native path and listing generation, and the final view restores interacted focus and the provisional scroll anchor by path. A pending parent-navigation target survives publication of the provisional page unless the user explicitly chooses another row. A sample gathered under an earlier filter is reprojected under the latest criteria. Widening the filter can reveal later matches when the complete listing arrives.

## Measurements

The Phase 2 comparison is revision `a305a59`, including the review fixes. The comparison and both Phase 3 runs used the same Intel Core i5-1145G7, Linux 6.8.0-149-generic x86-64, local ext4 fixture, rustc 1.96.0 and Cargo release profile. Auto-refresh was disabled. The plain fixture has 100,000 numeric filenames and the mixed fixture adds folders, links, comments and a hidden file. File creation preceded the warm or uncontrolled-cache runs. No directory snapshot cache participated. Resident sort and filter edits reuse data already loaded into the tab.

Timing cells are p50 / nearest-rank p95 in milliseconds unless otherwise noted. Navigation, stages and marked operations have five observations per configuration. The frame distribution pools the loading passes from five visits (628 Phase 2 and 918 Phase 3). The largest frame per visit is also shown to expose install stalls that pooled frame percentiles can hide.

| Metric | Phase 2 | Task 7 checkpoint | Phase 3 |
|---|---:|---:|---:|
| Navigation feedback draw, mixed 100k | 0.221 / 0.254 | 0.208 / 0.221 | 0.202 / 0.224 |
| First usable draw, mixed 100k | 238.12 / 263.74 | 248.85 / 272.59 | 4.40 / 4.56 |
| Complete egui draw, mixed 100k | 238.12 / 263.74 | 248.85 / 272.59 | 241.82 / 244.74 |
| Complete engine model, plain 100k | 222.60 / 245.66 | 232.33 / 237.12 | 237.77 / 241.41 |
| Longest engine poll per plain 100k visit | 73.457 / 81.895 | 0.027 / 0.033 | 0.042 / 0.131 |
| Foreground loading frame CPU, mixed 100k | 0.167 / 0.283 | 0.170 / 0.294 | 0.213 / 0.327 |
| Longest loading frame CPU per mixed 100k visit | 76.018 / 93.338 | 0.610 / 0.679 | 0.576 / 0.638 |
| Full initial Name sort, plain 100k | 71.70 / 79.85 | 72.77 / 75.96 | 74.56 / 76.86 |
| Initial projection, plain 100k | 1.35 / 1.84 | 3.33 / 3.65 | 2.99 / 3.29 |
| Final install, plain 100k | 73.446 / 81.885 | 0.002 / 0.002 | 0.015 / 0.017 |
| Resident mixed-100k Name reversal, dispatch | 23.463 / 24.293 | 0.003 / 0.003 | 0.003 / 0.004 |
| Resident mixed-100k Name reversal, complete | 23.47 / 24.30 | 41.09 / 46.55 | 40.85 / 43.04 |
| Resident mixed-100k glob filter, dispatch | 13.284 / 16.057 | 0.093 / 0.148 | 0.142 / 0.263 |
| Resident mixed-100k glob filter, complete | 13.29 / 16.06 | 23.89 / 27.29 | 24.17 / 26.07 |
| Full-list sorts per plain 100k visit | 1 | 1 | 1, plus 1 provisional sort |
| Directory scans during resident header sort | 0 | 0 | 0 |
| Plain 100k process peak RSS, one process | 44,680 KiB | 49,892 KiB | 57,820 KiB |
| Mixed UI process peak RSS, median of five | 61,604 KiB | 64,312 KiB | 64,296 KiB |

The first usable draw improved by 98%. Median full completion rose by about 7% in the plain engine run and 2% in the mixed UI run, while their measured tails held or fell. Fresh-name sorting remained close to Phase 2. Indirect lookup through the immutable raw order increased the time to complete the measured resident sort and filter. Their dispatch returned in less than 0.3 ms at p95. Global sort preferences now reproject resident data. The Phase 2 preferences path issued a refresh.

The first Task 7 implementation still spent 40–46 ms reconciling 100k marked paths in a refresh poll. After moving that work to the worker, the longest poll was 0.038 / 0.214 ms at the Task 7 checkpoint and 0.047 / 0.196 ms in Phase 3. A Phase 3 sort with all rows marked took 0.020 / 0.023 ms in its longest poll. The gated test also checks edits to marks while a result is pending.

In the marked-refresh trace, the inline capacity of live raw row vectors reached 48 MiB while old and new 100k snapshots overlapped. The plain single-pane trace reached 24 MiB plus one 12 KiB page. These counters exclude owned strings, indices, and buffers transferred to the retirement queue. Process peak RSS includes those allocations and allocator retention. The Task 7 and Phase 3 mixed engine runs, which also include marked refreshes, peaked at 157,596 KiB and 154,324 KiB respectively.

The headless harness times egui logic and draw submission. There was no window, monitor refresh rate or GPU presentation in these observations. Cold-cache runs would need isolated fixtures or cache controls. Native slow mounts, Windows and macOS performance remain unverified.

## Correctness checks

Formatting, 140 default workspace tests, 141 all-feature workspace tests, strict Clippy, the desktop release build, and both release harnesses passed. The feature-enabled gated UI test also passed separately against the 100k fixture. Tests cover superseded filter and sort requests, edits to marks during preparation, inactive view invalidation and a sort after the original directory was removed. The frozen comparator is exercised across natural sort, Unicode, direction, columns, and multi-run merges. Gated scan tests cover a usable first page, copying a known item, focus and marks through final reorder, blocking whole-directory commands, changed filters, navigating away, tab close, an empty directory, deletion during enumeration, terminal error after publication, and held refresh replacement. A feature-enabled egui test exercises Details, Compact, Gallery and Quick View while enumeration is gated. It also covers export-all feedback and pointer release after reordering. Existing workflow coverage includes recursive and virtual listings, archives, symlinks and Unix non-UTF-8 path identity.

## Reproduce

Generate the plain and mixed directories using the [Phase 1 fixture generator](performance-fixture.py). Build a comparison checkout with the navigation-frame and resident-sort timing changes from the test harness, excluding the Phase 3-only gated UI test, and the final checkout in separate Cargo target directories. Configure `DESHCMD_PERF_ROOT` to the same fixture for both builds. Build the test binaries using:

```bash
cargo test --release -p deshcmd-engine -p deshcmd-ui --features deshcmd-ui/perf --no-run
```

Use the paths printed by Cargo for the engine performance integration test and the UI unit-test binary. Run sequentially. The engine defaults to five visits per fixture and the UI comparison uses five fresh processes.

```bash
/usr/bin/time -v env DESHCMD_PERF_CASE=plain DESHCMD_PERF_STAGES=1 \
  "$ENGINE_TEST" --ignored --nocapture
/usr/bin/time -v env DESHCMD_PERF_CASE=100k DESHCMD_PERF_STAGES=1 \
  "$ENGINE_TEST" --ignored --nocapture
/usr/bin/time -v "$UI_TEST" --ignored --nocapture performance::phase1_ui
```

Set `DESHCMD_PERF_LARGE_TEST` to the fixture path when running the feature-enabled gated UI test to repeat the sort and filter interaction on 100k rows. Set `DESHCMD_PERF_MARKED_REFRESH=1` on the mixed engine run for the separate marked-refresh observation. Engine logs distinguish partial and complete installs. The UI logs record the first target-directory draw, the first complete draw, and CPU time for each loading pass.
