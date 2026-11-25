# Deshcmd Phase 2 performance results

## Architecture

The pre-Phase-2 revision, `61978bc`, included the Phase 1 scan, sort and selection improvements plus subsequent features such as archives. It spawned a thread for each listing, put complete listing results on the general event channel, and validated paths and history on the input thread. Places, disk space, tree expansion and several dialog reads also ran there.

- **Listings.** Two blocking workers take at most two queued requests, one for each visible pane. A new tab generation replaces obsolete work for that pane. A running read occupies a slot until it reaches a cancellation checkpoint. The pool limits the combined number of running and ready results to two. It wakes the consumer on the first ready result, and installs only results that still match the active tab ID and generation. Normal and recursive listing scans check cancellation between entries and before comments and metadata. Archive readers also receive the cancellation signal through their existing read control. The file-operation channel and job controls remain independent.
- **Navigation.** Two separate workers validate paths and history using the captured origin. Two pending slots are reserved for foreground intents within an eight-request queue. Explicit background tabs have independent request IDs. A failed or superseded request leaves the committed rows, marks and history in place. Escape cancels the active pane's foreground request. Commits compare the tab and request revisions. Focus and linked-peer changes require their captured revisions to remain current.
- **Ancillary reads.** Places use one worker, disk-space queries two, tree queries two, and dialog probes one. Space, places and tree results are keyed, coalesced and refreshed asynchronously. Failed probes wait two seconds before retrying, and a full queue backs off for 250 ms. Tree results share the folder vector and retain the first 251 folders, including the extra row used to signal that more folders exist. Native pickers and clipboard calls remain on their GUI thread. CLI directory validation starts after creating the app. Imported lists and workspace files, extra file properties, numbered-part searches, and transfer, split and bookmark destination checks use the dialog read worker. Dialog results check their originating tab and revision.

Dropping a pool cancels queued and running work and signals the workers to exit. The UI never joins a worker. A filesystem call already in the kernel can continue until the OS returns it.

## Measurements

Linux 6.8.0-149-generic x86-64, Intel Core i5-1145G7, local ext4, rustc 1.96.0 and Cargo release builds. Before and after used the same fixtures and workload. The plain 100k fixture was reused from Phase 1 on warm or uncontrolled OS caches. Its benchmark still times the complete view. The second fixture holds an actual scanner inside a comment-sidecar FIFO, then submits 100 navigations without polling, supersedes the burst, opens an accessible location in the other pane, and completes a file job before releasing the FIFO. The egui test measures logic and layout during a blocked listing at 1280 × 800 with a headless renderer. There is no GPU presentation or display refresh rate in that measurement.

Times are p50 / p95 in milliseconds. RSS is the median / largest process peak over five fresh runs. The 100k results contain 25 navigations per build. The stress dispatch, heartbeat and egui rows contain 500 samples per build. For single-per-process measurements, five observations make p95 the maximum.

| Metric | Phase 1 architecture | Phase 2 |
|---|---:|---:|
| Navigation dispatch, plain 100k | 0.0564 / 0.0655 ms | 0.0050 / 0.0070 ms |
| Navigation dispatch, FIFO burst | 0.0209 / 0.0435 ms | 0.0014 / 0.0023 ms |
| Headless egui frame while listing is blocked | 0.118 / 0.182 ms | 0.118 / 0.154 ms |
| Opposite pane ready during blocked I/O | 0.123 / 9.093 ms | 0.113 / 0.115 ms |
| Idle engine heartbeat during blocked I/O | 0.000029 / 0.000058 ms | 0.000109 / 0.000184 ms |
| Peak process threads, 100-request FIFO burst | 103 | 6 |
| Peak simultaneous listing work | 101 blocked scans in the burst | At most 2, observed with both workers gated |
| Maximum pending listing requests | Spawned immediately | 2, asserted with both workers gated |
| Obsolete metadata attempts per FIFO burst | 13,029 | 0 |
| Cancellation observed after supersession, including FIFO hold | No scan cancellation | 0.658 / 0.677 ms |
| Longest engine poll per plain 100k navigation | 91.26 / 97.34 ms | 87.23 / 92.80 ms |
| Plain 100k complete visible model | 237.84 / 252.56 ms | 242.97 / 251.14 ms |
| Peak RSS, FIFO burst | 11,776 / 12,416 KiB | 3,712 / 3,840 KiB |
| Peak RSS, plain 100k visits | 44,284 / 44,412 KiB | 44,552 / 44,808 KiB |

Headless steady frame time stayed close to the baseline while a listing was blocked. Before Phase 2, listing itself already ran off the UI thread. The separate gated tests establish that the additional navigation and dialog reads can also be held while the UI continues. Frequency of rare tail delays requires larger samples.

An intermediate implementation let idle workers race for serial work and reached 80,648 KiB in one plain 100k run. Preferring the first available worker preserves the second slot for concurrent work and brought the final serial RSS within about 1% of the baseline. The final complete-model median was similar to the baseline, and the 92.8 ms p95 poll still exceeds the 16.7 ms frame budget.

The queue bound is an assertion under gated concurrent execution. The old implementation spawned immediately, so obsolete work accumulated as in-flight threads and complete results instead of pending application requests. Peak process threads include the benchmark threads and idle workers. Obsolete metadata attempts are the scan-hook counts in the FIFO scenario. Cancellation latency in that scenario includes the time the benchmark deliberately keeps the FIFO blocked after supersession.

## Correctness and limitations

The tests hold the pool behind gates to verify that 100 replacement requests stay within two running and two pending slots, that releasing a stale read does not publish its result, and that shutdown does not wait for a blocked syscall. A real FIFO exercises listing supersession on Linux. Gated navigation tests cover preserved marks, newer failures, linked-peer revision checks, tab close, workspace restore, and overlapping Back and Forward. UI tests exercise rendering and navigation with a dialog read blocked, stale dialog results, duplicate suppression, and retry after a probe failure. Existing archive, linked-pane, tab, selection, file job and conflict tests remain in the workspace suite.

Formatting, all 120 workspace tests, strict Clippy with default and all features, and the debug and release desktop builds passed. The release engine and UI harnesses passed. The focused Linux/X11 smoke check verified CLI startup, asynchronous new-tab navigation, clean shutdown and workspace restoration. A broader preexisting smoke script stopped at its application-menu installation assertion. That menu workflow was outside this phase.

Complete listing sort, projection and old-row destruction still run during engine polling. An uncancellable read can occupy one worker and two such reads can occupy both slots in a pool. Ancillary pools cannot hold listing or validation slots. Native pickers, explicit unmount, and clipboard integration retain their existing threading. No slow network mount was available, so the blocked-read exercise used a local FIFO and test barriers. Windows drive and UNC behavior, macOS volume behavior, native presentation latency, and cold-cache performance remain release validation items.

The next plan dependency is Task 7, moving full view preparation off the input thread. Task 8 can build on that to publish a usable first page. Neither change was included in Phase 2.

## Reproduce

Build each revision in a separate Cargo target directory. Copy `crates/deshcmd-engine/tests/concurrency_performance.rs` to the pre-Phase-2 checkout. For its UI run, append only the `phase2_blocked_listing_ui` test from `crates/deshcmd-ui/src/performance.rs` to the pre-Phase-2 version of that file.

```bash
cargo test --release -p deshcmd-engine --features perf --test concurrency_performance --no-run
cargo test --release -p deshcmd-engine --features perf --test performance --no-run
cargo test --release -p deshcmd-ui --features perf --lib --no-run
```

Use the executable paths printed by Cargo as `STRESS_TEST`, `ENGINE_TEST` and `UI_TEST`. Run each command in a separate process five times per revision. The fixture generator and plain directory protocol are in the [Phase 1 report](performance-phase1-results.md#reproduce).

```bash
/usr/bin/time -v env DESHCMD_PERF_STAGES=1 \
  "$STRESS_TEST" --ignored --nocapture
/usr/bin/time -v env DESHCMD_PERF_STAGES=1 DESHCMD_PERF_CASE=plain \
  "$ENGINE_TEST" --ignored --nocapture
"$UI_TEST" --ignored --nocapture performance::phase2_blocked_listing_ui
```
