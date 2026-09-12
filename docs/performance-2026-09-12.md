# Cutting Board performance audit — 2026-09-12

This document records the measurements and code review completed during the 2026-09-12
performance pass. It is an audit of the current repository and measured execution paths; it does
not claim that every possible usability or performance issue has been exhausted.

## Environment

- macOS Darwin 25.6.0, arm64.
- Node.js v24.15.0 and npm 11.12.1.
- Rust/Cargo 1.98.1.
- The already-running stable app was measured as PID 13012 from
  `~/Applications/Cutting Board.app/Contents/MacOS/cutting-board`.

## Audit coverage

| Area | Review or measurement | Result/status |
| --- | --- | --- |
| Scanner | Reviewed targeted listener refresh and bounded ancestor refresh. | The targeted process set preserves listener records and loads the ancestors needed by `origin_for`. A fake-`lsof` fixture on a host with 819 processes, one fixture listener, one endpoint, and four ancestors measured p50 31.507ms → 14.410ms and p95 52.580ms → 25.873ms over 30 interleaved samples, with stable-field parity. |
| System metrics | Reviewed existing polling and reused `System` behavior. | No new code change or standalone timing was recorded in this pass. |
| Docker | Reviewed the Docker CLI/fallback paths and existing parser tests. | Both installed-app smoke checks listed four Docker entries successfully. A CLI permission denial occurred only inside the tool sandbox and was not treated as an application defect. No standalone Docker timing was recorded. |
| Frontend logs | Reviewed CRLF normalization, visual line-break offsets, selection mapping, append reuse, and rolling-tail behavior. | Output and offsets matched the previous implementation. Selection mapping improved from 1.351ms → 0.692ms at 64KiB and 2.436ms → 1.336ms at 96KiB. The typed KMP buffer used about half the measured V8 backing storage. Selection, scroll, and focus regression tests passed. |
| Launch logs | Measured process-command provenance and managed-log recovery against the previous implementations. | Provenance targeted refresh was adopted: median 4.676875ms → 0.110333ms and p95 5.976083ms → 0.150917ms. Recovery with eight profiles and eight approximately 1.15MB files improved p50 52.114709ms → 7.925791ms and p95 60.859167ms → 8.847834ms. The common one-profile case was 0.823542ms → 0.826875ms at the median, within measurement noise. |
| Storage | Reviewed existing storage tests, atomic writes, and call sites. | Geometry storage was measured through the actual settings load/save path; see Window. |
| Process control | Reviewed the targeted identity refresh and restart/termination paths. | No new code change or standalone timing was recorded in this pass. |
| Window | Reviewed geometry persistence, migration, and coalescer lifecycle tests. | A release coalescer benchmark measured 409.216ms for 100 synchronous writes versus 0.004ms enqueue + 3.892ms flush for one write. Concurrent settings writes, repeated geometry during an in-flight save, duplicate flush, write failure, and pending writes at shutdown were tested. |
| Update | Reviewed update polling, progress, and build helper tests. | No new code change or standalone timing was recorded in this pass. |

## Verified changes and decisions

The launch-log provenance lookup now refreshes only the current process and its parent chain. The
release benchmark retained the same command list while reducing the measured refresh cost by
about 42.4× at the median and 39.6× at p95. Managed-log recovery also reuses the directory scan
and marker metadata, reducing the measured p50 from 52.114709ms to 7.925791ms and p95 from
60.859167ms to 8.847834ms in the larger fixture. The common one-profile case remained within
noise at 0.823542ms to 0.826875ms. The scanner uses the same targeted-refresh principle for
listener processes and refreshes only the command metadata needed by its bounded ancestor walk;
its fixture result now has stable service, endpoint, process identity, project, and provenance
parity.

The console presentation check covered CRLF normalization, appended output, manual visual
separators, rolling tails, and Unicode strings. A randomized run of 5,000 scenarios with 50
updates per scenario found no output or offset mismatch against the pre-optimization behavior.
The retained console benchmark reports the selection-mapping improvement above and shows the
typed KMP buffer using about half the backing storage measured for the old `Array<number>` buffer.

The manual-break append reuse experiment was rejected and removed from the accepted optimization
set. Its one-off Node string-path measurements were effectively noise: 64KiB append 0.042ms →
0.043ms, 96KiB 0.062ms → 0.060ms, 512KiB 0.313ms → 0.311ms, and 2MiB 1.271ms → 1.346ms. The
experiment was not retained as a repository benchmark, so these figures are historical evidence
and are not presented as a reproducible command. The measurement also excluded browser DOM
layout, textarea assignment, and native IPC, so it could not establish a user-visible win for
that path.

The measured implementation units were committed as follows: `597bd45` for macOS canonical-path
fixtures, `4944b4b` for targeted process provenance, `9ce5c46` for console selection mapping and
typed KMP storage, `f317c9d` for targeted scanner refresh, `c909cfc` for managed-log recovery, and
`0d6b6e6` for window geometry coalescing.

## Baseline checks

The frontend checks completed successfully:

- Initial baseline `npm test`: 128/128 tests passed; command real time 0.46s and test-reported duration 312.97ms. The later integration suite reached 130/130 tests passed.
- `npm run check`: passed; command real time 1.03s, including the generated-icon precheck. The later typecheck also passed.

The first Rust run completed 126/131 tests. The five failures compared `/private/var/...` paths
with `/var/...` expectations on macOS. Canonicalizing those temporary test paths preserved the
behavior under test; the corrected intermediate suite completed 132/132 tests with command
real time 7.14s and test-reported library duration 4.37s. The final serialized Rust suite later
completed 146 tests with 3 ignored. The test environment also emitted unrelated shell startup
write-permission warnings for the user's cache, zsh dump, and fnm multishell paths.

## Commands and methodology

The baseline commands were:

```text
/usr/bin/time -p npm test
/usr/bin/time -p npm run check
/usr/bin/time -p cargo test --manifest-path src-tauri/Cargo.toml
```

The final integration commands were:

```text
npm test
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --offline --manifest-path src-tauri/Cargo.toml --all-targets
```

All passed: 130 frontend tests and 146 Rust tests, with 3 manual performance benchmarks
intentionally ignored by the normal Rust suite. Those benchmarks were run explicitly in release
mode during this audit. The final Rust suite used its normal parallel execution and completed in
4.58 seconds. Non-fatal fnm sandbox warnings did not cause test failures.

The release launch-log provenance benchmark command was:

```text
/usr/bin/time -p env TMPDIR=/tmp cargo test --release --manifest-path src-tauri/Cargo.toml launch::logs::tests::benchmark_process_command_provenance_single_process -- --ignored --nocapture
```

It ran 101 paired samples for the old full process refresh and the targeted ancestor refresh in a
release profile, sorted the samples, and reported the median and p95. Both implementations had to
return the same command list on every sample. The raw output is in
`/tmp/cutting-board-process-provenance-release.txt`.

The managed-log recovery benchmark command was:

```text
/usr/bin/time -p env TMPDIR=/tmp cargo test --release --manifest-path src-tauri/Cargo.toml launch::logs::tests::benchmark_recovered_managed_log_path_reuses_directory_and_markers -- --ignored --nocapture
```

It compared the previous full directory and marker scan with the directory-level and marker
metadata reuse path over 21 interleaved samples. The larger fixture used eight profiles and eight
approximately 1.15MB log files; the common case used one profile. Both implementations had to
return identical paths.

The retained console benchmark command is:

```text
npm run bench:console-log-view
```

It compares the exact previous and current selection-mapping algorithms and console presentation
transitions at 64KiB, 96KiB, 512KiB, and 2MiB rolling fixtures. It checks output, line-break
offsets, and selection mappings. Small fixtures exhaustively compare start/end offsets across
empty, unchanged, appended, replaced, truncated, repeated, and Unicode/CRLF/ANSI inputs. Larger
rolling fixtures compare boundary selections. The earlier randomized append experiment was a
separate temporary harness, not part of this retained benchmark. Node timings exclude WebKit
layout, textarea assignment, and Tauri IPC.

The scanner fixture benchmark command is:

```text
node scripts/bench-scanner.mjs
```

It builds a temporary release-profile crate from the production scanner, replaces only the
selective process refresh with the prior full refresh for the baseline, runs a fake-`lsof` fixture
with one listener and one endpoint, and compares 30
interleaved scans. It asserts stable service, endpoint, process identity, project, and provenance
fields before reporting timings. Host process and ancestor counts vary between runs; the reported
run had 819 processes and four ancestors. The fixture replaces listener discovery rather than
measuring the real system-wide `lsof` command. CPU, memory, and uptime fields are excluded from
equality checks because they can change between calls.

The window storage benchmark command is:

```text
/usr/bin/time -p env TMPDIR=/tmp cargo test --release --manifest-path src-tauri/Cargo.toml window::tests::benchmark_geometry_storage_burst -- --ignored --nocapture
```

It compares 100 synchronous geometry writes with one coalesced flush and reports enqueue and
flush durations. The release measurement was 409.216ms for 100 writes versus 0.004ms enqueue +
3.892ms flush for one write.

The stable app's native baseline used two `ps` samples around a 20-second interval. CPU time was
1:16.30 → 1:16.56 (about +0.26s), and RSS was 70,208KB → 70,304KB (+96KB). Raw samples are in
`/tmp/cutting-board-native-baseline-20s.txt`.

After installation and reopening the Launch task log, a 20-second sample of the new native
process recorded CPU time 0:01.05 → 0:01.29 (+0.24s) and RSS 106,144KB → 106,384KB. Raw samples
are in `/tmp/cutting-board-native-after-20s.txt`. The CPU difference from the baseline is too small
to establish a whole-app improvement. RSS was higher after restart; background processes and the
recovered log source also differed. These observations do not establish a whole-app memory
reduction. The causal performance claims in this report are limited to the paired benchmarks.

## Application build and smoke verification

- `npm run tauri build -- --bundles app` passed with the existing Apple Development identity.
- The fresh bundle was synced to `~/Applications/Cutting Board.app` after terminating old app
  instances, then launched from that stable path.
- `codesign --verify --deep --strict` passed. The installed executable and the build executable
  had identical SHA-256 hashes.
- Services, Docker, and Launch tabs rendered successfully; existing running tasks and their logs
  were visible after the restart.
- A native window resize updated the saved geometry. Quit/reopen retained the resulting size and
  position, as well as the existing theme and scan interval. The final app remains running from
  the stable installed path.
- Notarization was not run because notarization credentials were not configured; the local
  development signature was verified.

## Limitations and remaining candidates

The benchmarks ran on one macOS host with concurrent background workload. The
native baseline measures only the app process and does not isolate other system activity. The
console experiment measured JavaScript string handling and excluded browser layout, textarea
selection/scroll work, and Tauri IPC. Docker,
system-metrics, process control, and update paths were reviewed but have no standalone
performance number in this report; they remain candidates for focused measurement rather than
evidence that every possible issue in those areas has been exhausted.

Full launch snapshots still include every task's log tail. Replacing them with selected-task
polling needs a separate contract for managed, external-file, and container logs and was not
adopted without equivalent behavior measurements. Long preparation operations still hold the
launch manager lock; the existing direct log-tail path remains available during preparation.
Geometry write failures are logged and can be retried by a later geometry event or close, as with
the previous error behavior. Linux execution and a controlled whole-WebView CPU/latency comparison
were not verified in this macOS session.
