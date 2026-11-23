# Deshcmd Phase 1 performance results

The baseline was captured at `cc30d40` with only the benchmark, fixture and opt-in timing hooks applied, before changing selection, the scanner or projection. The baseline, Task 2 checkpoint and final candidate were measured on the same Intel Core i5-1145G7 (4 cores, 8 hardware threads), 30 GiB RAM, Linux 6.8.0-149-generic x86-64, local ext4, rustc 1.96.0 and Cargo's release profile. Fixtures were created before timing. The OS cache was warm or uncontrolled and auto-refresh was disabled. Each visit loaded a new application listing. “First” refers to the first visit in the test process.

## Results

Times are medians in milliseconds unless stated otherwise. The 100k table uses the plain fixture for scan, initial sort/projection, install, poll, model completion and RSS. It uses the mixed fixture for the glob filter and egui measurements.

| Metric | Before | After | Improvement |
|---|---:|---:|---:|
| 100k directory enumeration and scanner bookkeeping, estimated | 69.05 ms | 27.54 ms | 60.1% |
| Metadata and row materialization, estimated | 140.28 ms | 127.81 ms | 8.9% |
| Complete directory scan | 209.36 ms | 157.86 ms | 24.6% |
| Sorting, initial Name sort | 156.79 ms | 86.33 ms | 44.9% |
| Filtering, mixed typed-glob dispatch | 47.61 ms | 16.04 ms | 66.3% |
| Initial empty-text projection | 58.96 ms | 1.57 ms | 97.3% |
| Listing installation, including sorting and projection | 210.41 ms | 88.03 ms | 58.2% |
| Longest engine poll per plain navigation | 210.42 ms | 88.04 ms | 58.2% |
| Request to complete visible model | 435.11 ms | 246.21 ms | 43.4% |
| Steady-state frame work, all marked and cursor at end | 42.97 ms | 0.149 ms | 99.7% |
| Memory usage, plain process peak RSS | 61,648 KiB | 43,400 KiB | 29.6% |

The plain 100k p95 was 468.05 → 253.08 ms for navigation and 250.79 → 88.11 ms for the longest poll. Five observations make nearest-rank p95 the maximum. Worker start after scheduling was 0.118 → 0.081 ms at the median. The first materialized row was available to the worker at 29.12 → 0.225 ms after scan start. The worker published its complete batch at 224.54 → 157.98 ms after scheduling. The first populated egui draw in the mixed 100k run was submitted at 466.73 → 250.89 ms. Loading feedback was submitted at 1.20 → 0.95 ms, and the following settled draw at 467.05 → 251.26 ms. Those draw milestones are single observations. There was no window, refresh rate or GPU presentation in the timing loop.

### Navigation across fixture sizes

The first visit is a single measurement, revisits are the median of four A→B→A visits, and the poll column includes all five visits with p95 in parentheses.

| Mixed fixture | First visit, before → after | Revisit, before → after | Longest poll median (p95), before → after |
|---|---:|---:|---:|
| 100 files | 0.45 → 0.30 | 0.50 → 0.25 | 0.12 (0.23) → 0.04 (0.05) |
| 10,000 files | 39.98 → 20.41 | 37.34 → 21.38 | 15.34 (19.13) → 3.99 (4.45) |
| 100,000 files | 485.60 → 253.32 | 444.99 → 244.99 | 212.57 (257.31) → 88.85 (89.50) |

The mixed empty-to-empty reference was 0.062 → 0.078 ms. Single mixed 100k scenarios took 635.20 → 333.31 ms to install two panes at the same path, 436.98 → 298.49 ms for recursive search, and 406.13 → 246.08 ms while an actual transfer waited on a collision. The two-pane poll reached 169.43 ms after the changes. Dispatching away from 100k marked paths took 18.91 → 17.70 ms at the median. Mark All itself took 15.35 → 19.46 ms (p95 28.62 → 21.57 ms), including construction of the mark set and its byte total. At 100k, switching to Size took 106.69 → 19.44 ms, reversing Name took 96.11 → 28.91 ms, and clearing the filter took 54.85 → 1.26 ms. The typed glob's p95 was 61.89 → 16.72 ms.

### Steady frames and export

The test runs the app's egui logic and layout, with the footer enabled, at 1280 × 800. Each cell contains median (p95) over 50 passes on mixed 100k unless noted.

| Workload | Before | Task 2 checkpoint | Final |
|---|---:|---:|---:|
| No marks, cursor at head | 0.152 (0.184) | 0.135 (0.191) | 0.151 (0.176) |
| No marks, cursor at end | 9.087 (12.297) | 0.133 (0.136) | 0.148 (0.153) |
| One mark, cursor at end | 21.525 (24.261) | 0.137 (0.193) | 0.145 (0.183) |
| All marked, cursor at end | 42.973 (47.950) | 0.138 (0.154) | 0.149 (0.173) |
| All marked, cursor and viewport near the middle | 42.702 (45.993) | 0.137 (0.210) | 0.155 (0.187) |
| Wheel scroll with all marked | 43.979 (48.192) | 0.146 (0.192) | 0.149 (0.179) |
| Export all, 10k, Name column, five runs | 2077.09 (2082.00) | 0.44 (1.30) | 0.63 (1.39) |

The regular cursor-step benchmark runs 100 consecutive steps near the end with every path marked and queries the focused entry after each step. Its median was 1637.87 ms before and about 0.004 ms at both the Task 2 checkpoint and the final candidate. The final 100k Name-only export took 13.59 ms at the median. The 100k baseline export was omitted because the original 10k export already took roughly two seconds per run. Export timings end at the save-picker boundary.

### Allocation and syscall observations

- On the plain scan, the hooks counted 100,000 metadata attempts and one comment-map read in both builds. Deduplication path clones fell from 100,000 to zero. Streaming increased the final row vector's inline capacity from 18,400,000 to 24,117,248 bytes on this fixture. That counter covers the row buffer's reserved capacity. Process RSS also reflects resident heap pages, allocator behavior, owned strings and paths, and temporary data.
- Plain scan syscall counts were identical in separate Linux tracing runs with one 100k visit. Both recorded 100,021 `statx`, 129 `getdents64` and 21 `openat` calls, including process startup, empty navigation and the return from 100k. The Linux gain came from reduced application work. Windows metadata reuse was unmeasured.
- From the code paths and fixture, the baseline empty-text projection normalized 100,000 names per plain visit. The candidate normalizes zero. Each baseline name comparison created two lowercase strings. The candidate prepares 100,000 borrowed keys on the ASCII lowercase plain fixture, with zero lowercase string allocations. Total allocator calls and individual sort comparison counts were unmeasured.
- The mixed process peak RSS, including two simultaneous listings, was 138,896 → 109,324 KiB. The headless UI process peak was 71,904 → 60,432 KiB. Each is one process high-water measurement. The profiler reports one scan result's inline row capacity. It does not measure queued heap bytes or ordinary queue-depth high-water.
- The FIFO workflow test waits for two startup publications before polling and later observes one stale scan publication after a newer result installs. Cooperative cancellation is outside Phase 1. The local FIFO verified ordering under a stalled read. No slow or remote mounted filesystem was available.

## Reproduce

The [fixture generator](performance-fixture.py) creates 100, 10,000 and 100,000 scrambled numeric filenames, plus a child folder, dotfile, comment sidecar and two symlinks where supported. The separate plain fixture has 100,000 empty files for comparison with the original headless protocol.

```bash
run=$(mktemp -d /tmp/deshcmd-perf.XXXXXX)
python3 docs/performance-fixture.py "$run/fixture"
export DESHCMD_PERF_ROOT="$run/fixture"
cargo test --release -p deshcmd-engine -p deshcmd-ui --features deshcmd-ui/perf --no-run
```

Set `ENGINE_TEST` to the `tests/performance.rs` executable and `UI_TEST` to the `deshcmd-ui` unit-test executable printed by Cargo. Run the test binaries directly so RSS measures the benchmark process. Run sequentially and use the same fixture, flags and binaries built with the same compiler profile for each comparison.

```bash
/usr/bin/time -v -o "$run/mixed.rss" env DESHCMD_PERF_STAGES=1 \
  "$ENGINE_TEST" --ignored --nocapture > "$run/mixed.log" 2>&1
/usr/bin/time -v -o "$run/plain.rss" env DESHCMD_PERF_STAGES=1 DESHCMD_PERF_CASE=plain \
  "$ENGINE_TEST" --ignored --nocapture > "$run/plain.log" 2>&1
/usr/bin/time -v -o "$run/ui.rss" \
  "$UI_TEST" --ignored --nocapture performance::phase1_ui > "$run/ui.log" 2>&1
DESHCMD_PERF_CASE=plain DESHCMD_PERF_RUNS=1 \
  strace -f -c -e trace=%file,getdents64 -o "$run/strace.txt" \
  "$ENGINE_TEST" --ignored --nocapture > "$run/trace-run.log" 2>&1
```

`sample` lines contain outer timings in milliseconds. `begin` labels the scenario and `stage` lines contain internal stage timings in milliseconds. For the initial stages use the records following `begin,plain/first` or `begin,plain/repeat` up to the corresponding `poll_max` sample. The hook measures roughly 1 in 128 row materializations. Enumeration and bookkeeping is the rest of the scan time after subtracting the estimated materialization time. Interleaving enumeration and row extraction places some path construction inside the sampled region in the candidate. Use `DESHCMD_PERF_EXPORT_100K=1` on the UI run for the additional large export.

## Verification

`cargo fmt --all -- --check`, `cargo test --workspace` (14 tests), `cargo clippy --workspace --all-targets --all-features` and the desktop release build passed. Both release harnesses passed. The Linux/X11 smoke test exercised filtering, copying, folder creation, Quick View and dragging two marked operands. Its scripted drag used coordinates in the left pane, so the copied directory names were verified against the drag dialog and on disk.

## Remaining targets

The headless steady-frame p95 is below 16.7 ms for the measured interactions. The 100 ms first-usable-row target and the 16.7 ms poll budget during large installs remain open. The final scanner still performs metadata work for each entry on Linux. The input thread still sorts complete batches, can install two in a single poll, and drops old rows and marks on navigation.

For Phase 2, prioritize bounded stale-scan work (Task 3), then versioned worker-side sort and projection (Task 7). Task 8 can deliver a usable first page after asynchronous navigation validation (Task 4) and versioned installation are in place. Release evaluation still needs cold-cache isolation, native presentation and input latency, slow or remote storage, and Windows and macOS verification.
