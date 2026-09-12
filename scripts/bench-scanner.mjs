#!/usr/bin/env node

/**
 * Build and run an apples-to-apples scanner benchmark.
 *
 * The harness copies the production scanner and its child modules into a
 * temporary crate. The baseline copy is made from that exact source by
 * replacing only the selective process refresh call with the old full-process
 * refresh. A fake lsof reports the benchmark process as one listener, so both
 * implementations inspect the same PID and endpoint on every sample.
 */

import { chmod, cp, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const scannerRoot = join(repositoryRoot, "src-tauri", "src", "scanner");
const scannerPath = join(repositoryRoot, "src-tauri", "src", "scanner.rs");
const childModules = [
  "classification.rs",
  "demo.rs",
  "listeners.rs",
  "presentation.rs",
  "project.rs",
  "spring.rs",
  "tests.rs",
];

const benchmarkMain = String.raw`mod models {
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Endpoint {
        pub family: String,
        pub address: String,
        pub port: u16,
        pub scope: String,
        pub protocol: String,
    }

    #[derive(Debug, Clone)]
    pub struct ProcessInfo {
        pub pid: u32,
        pub parent_pid: Option<u32>,
        pub name: String,
        pub executable: Option<String>,
        pub working_directory: Option<String>,
        pub command: String,
        pub launch_command: Option<String>,
        pub create_time: u64,
        pub uptime_seconds: u64,
        pub cpu_percent: Option<f32>,
        pub memory_bytes: Option<u64>,
        pub uid: Option<u32>,
    }

    #[derive(Debug, Clone)]
    pub struct ProjectInfo {
        pub id: String,
        pub name: String,
        pub root_path: String,
        pub detection_source: String,
        pub workspace_root_path: String,
        pub workspace_name: String,
    }

    #[derive(Debug, Clone)]
    pub struct ServiceSnapshot {
        pub id: String,
        pub display_name: String,
        pub tech: String,
        pub category: String,
        pub relevance: String,
        pub endpoints: Vec<Endpoint>,
        pub process: Option<ProcessInfo>,
        pub project: Option<ProjectInfo>,
        pub status: String,
        pub warnings: Vec<String>,
        pub origin_kind: String,
        pub origin_label: Option<String>,
        pub can_terminate: bool,
        pub browser_url: Option<String>,
        pub active_profiles: Vec<String>,
    }

    #[derive(Debug, Clone)]
    pub struct WorkspaceSnapshot {
        pub services: Vec<ServiceSnapshot>,
        pub scanned_at: u64,
        pub scan_duration_ms: u128,
        pub endpoint_count: usize,
        pub errors: Vec<String>,
    }

    #[derive(Debug, Clone)]
    pub struct ServiceIdentity {
        pub pid: u32,
        pub start_time: u64,
        pub uid: Option<u32>,
        pub display_name: String,
    }

    pub fn now_epoch() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

#[path = "scanner_before/mod.rs"]
mod scanner_before;
#[path = "scanner_after/mod.rs"]
mod scanner_after;

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

fn main() {
    let fixture = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
    );
    let mut ancestors = 0;
    let mut seen = HashSet::new();
    let mut parent = fixture
        .process(Pid::from_u32(std::process::id()))
        .and_then(|process| process.parent());
    while let Some(pid) = parent {
        if ancestors == 10 || !seen.insert(pid) {
            break;
        }
        ancestors += 1;
        parent = fixture.process(pid).and_then(|process| process.parent());
    }
    println!(
        "fixture total_processes={} listener_pids=1 listener_endpoints=1 origin_ancestors={ancestors}",
        fixture.processes().len(),
    );

    let mut baseline = Vec::new();
    let mut selective = Vec::new();
    for index in 0..30 {
        let (baseline_elapsed, expected, selective_elapsed, actual, order) = if index % 2 == 0 {
            let started = Instant::now();
            let expected = scanner_before::scan_workspace(false).expect("baseline scan failed");
            let baseline_elapsed = started.elapsed();
            let started = Instant::now();
            let actual = scanner_after::scan_workspace(false).expect("selective scan failed");
            (baseline_elapsed, expected, started.elapsed(), actual, "baseline-first")
        } else {
            let started = Instant::now();
            let actual = scanner_after::scan_workspace(false).expect("selective scan failed");
            let selective_elapsed = started.elapsed();
            let started = Instant::now();
            let expected = scanner_before::scan_workspace(false).expect("baseline scan failed");
            (started.elapsed(), expected, selective_elapsed, actual, "selective-first")
        };

        assert_equivalent(&expected.0, &actual.0);
        assert_eq!(expected.1.len(), actual.1.len(), "scan index {index} identity mismatch");
        for (id, expected) in &expected.1 {
            let actual = actual.1.get(id).expect("missing identity");
            assert_eq!(expected.pid, actual.pid, "identity pid mismatch for {id}");
            assert_eq!(expected.start_time, actual.start_time, "identity start mismatch for {id}");
            assert_eq!(expected.uid, actual.uid, "identity uid mismatch for {id}");
            assert_eq!(expected.display_name, actual.display_name, "identity name mismatch for {id}");
        }
        assert_eq!(actual.0.services.len(), 1, "fixture service count changed");
        assert_eq!(actual.0.endpoint_count, 1, "fixture endpoint count changed");
        baseline.push(baseline_elapsed);
        selective.push(selective_elapsed);
        println!(
            "pair={index} order={order} baseline_ms={:.3} selective_ms={:.3} services={} endpoints={} equivalent=true",
            baseline_elapsed.as_secs_f64() * 1000.0,
            selective_elapsed.as_secs_f64() * 1000.0,
            actual.0.services.len(),
            actual.0.endpoint_count,
        );
    }
    report("baseline", &mut baseline);
    report("selective", &mut selective);
}

fn report(name: &str, durations: &mut [Duration]) {
    durations.sort_unstable();
    let p50 = durations[durations.len() / 2].as_secs_f64() * 1000.0;
    let p95 = durations[(durations.len() * 95 / 100).min(durations.len() - 1)].as_secs_f64()
        * 1000.0;
    let mean = durations.iter().map(|time| time.as_secs_f64()).sum::<f64>() * 1000.0
        / durations.len() as f64;
    println!(
        "summary={name} n={} mean_ms={mean:.3} p50_ms={p50:.3} p95_ms={p95:.3}",
        durations.len()
    );
}

fn assert_equivalent(
    expected: &models::WorkspaceSnapshot,
    actual: &models::WorkspaceSnapshot,
) {
    assert_eq!(expected.endpoint_count, actual.endpoint_count);
    assert_eq!(expected.errors, actual.errors);
    assert_eq!(expected.services.len(), actual.services.len());
    for (expected, actual) in expected.services.iter().zip(&actual.services) {
        assert_eq!(expected.id, actual.id);
        assert_eq!(expected.display_name, actual.display_name);
        assert_eq!(expected.tech, actual.tech);
        assert_eq!(expected.category, actual.category);
        assert_eq!(expected.relevance, actual.relevance);
        assert_eq!(expected.endpoints, actual.endpoints);
        assert_project_equivalent(expected.project.as_ref(), actual.project.as_ref());
        assert_eq!(expected.status, actual.status);
        assert_eq!(expected.warnings, actual.warnings);
        assert_eq!(expected.origin_kind, actual.origin_kind);
        assert_eq!(expected.origin_label, actual.origin_label);
        assert_eq!(expected.can_terminate, actual.can_terminate);
        assert_eq!(expected.browser_url, actual.browser_url);
        assert_eq!(expected.active_profiles, actual.active_profiles);
        match (expected.process.as_ref(), actual.process.as_ref()) {
            (None, None) => {}
            (Some(expected), Some(actual)) => {
                assert_eq!(expected.pid, actual.pid);
                assert_eq!(expected.parent_pid, actual.parent_pid);
                assert_eq!(expected.name, actual.name);
                assert_eq!(expected.executable, actual.executable);
                assert_eq!(expected.working_directory, actual.working_directory);
                assert_eq!(expected.command, actual.command);
                assert_eq!(expected.launch_command, actual.launch_command);
                assert_eq!(expected.create_time, actual.create_time);
                assert_eq!(expected.uid, actual.uid);
                // Uptime, CPU, and memory are expected to change between calls.
            }
            _ => panic!("process presence changed"),
        }
    }
}

fn assert_project_equivalent(
    expected: Option<&models::ProjectInfo>,
    actual: Option<&models::ProjectInfo>,
) {
    match (expected, actual) {
        (None, None) => {}
        (Some(expected), Some(actual)) => {
            assert_eq!(expected.id, actual.id);
            assert_eq!(expected.name, actual.name);
            assert_eq!(expected.root_path, actual.root_path);
            assert_eq!(expected.detection_source, actual.detection_source);
            assert_eq!(expected.workspace_root_path, actual.workspace_root_path);
            assert_eq!(expected.workspace_name, actual.workspace_name);
        }
        _ => panic!("project presence changed"),
    }
}
`;

async function main() {
  if (process.platform === "win32") {
    throw new Error("bench-scanner.mjs requires a Unix shell for its fake lsof fixture");
  }

  const scannerSource = await readFile(scannerPath, "utf8");
  const selectiveImport =
    "use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};";
  const baselineImport =
    "use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind};";
  const selectiveSetup =
    "let mut system = System::new();\n    refresh_processes_for_scan(&mut system, process_pids);";
  const baselineSetup =
    "let system = System::new_with_specifics(\n        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything().without_tasks()),\n    );";

  if (!scannerSource.includes(selectiveImport)) {
    throw new Error("scanner.rs no longer has the expected sysinfo import; update this benchmark");
  }
  if (!scannerSource.includes(selectiveSetup)) {
    throw new Error("scanner.rs no longer has the selective refresh call; update this benchmark");
  }
  const baselineSource = scannerSource
    .replace(selectiveImport, baselineImport)
    .replace(selectiveSetup, baselineSetup);
  if (baselineSource === scannerSource || !baselineSource.includes("RefreshKind::nothing()")) {
    throw new Error("failed to construct the exact full-process-refresh baseline");
  }

  const temporaryRoot = await mkdtemp(join(tmpdir(), "cutting-board-scanner-bench-"));
  try {
    const sourceRoot = join(temporaryRoot, "src");
    await mkdir(sourceRoot, { recursive: true });
    await writeFile(
      join(temporaryRoot, "Cargo.toml"),
      `[package]\nname = "cutting-board-scanner-bench"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\nserde_json = "1"\nsha1 = "0.10"\nsysinfo = "0.39"\n`,
    );
    await writeFile(join(sourceRoot, "main.rs"), benchmarkMain);

    for (const [name, source] of [
      ["scanner_before", baselineSource],
      ["scanner_after", scannerSource],
    ]) {
      const moduleRoot = join(sourceRoot, name);
      await mkdir(moduleRoot, { recursive: true });
      await writeFile(join(moduleRoot, "mod.rs"), source);
      for (const childModule of childModules) {
        await cp(join(scannerRoot, childModule), join(moduleRoot, childModule));
      }
    }

    const fakeBin = join(temporaryRoot, "bin");
    await mkdir(fakeBin);
    await writeFile(
      join(fakeBin, "lsof"),
      "#!/bin/sh\nprintf 'p%s\\ncbench-listener\\nu%s\\nPTCP\\nn127.0.0.1:3000\\n' \"$PPID\" \"$(id -u)\"\n",
    );
    await chmod(join(fakeBin, "lsof"), 0o755);

    const manifest = join(temporaryRoot, "Cargo.toml");
    const build = spawnSync(
      "cargo",
      ["build", "--offline", "--release", "--manifest-path", manifest],
      { cwd: temporaryRoot, stdio: "inherit" },
    );
    if (build.error) {
      throw build.error;
    }
    if (build.status !== 0) {
      throw new Error(`benchmark build failed with exit code ${build.status}`);
    }

    const executable = join(temporaryRoot, "target", "release", "cutting-board-scanner-bench");
    const run = spawnSync(executable, [], {
      cwd: temporaryRoot,
      env: { ...process.env, PATH: `${fakeBin}:/usr/bin:/bin` },
      stdio: "inherit",
    });
    if (run.error) {
      throw run.error;
    }
    if (run.status !== 0) {
      throw new Error(`benchmark run failed with exit code ${run.status}`);
    }
  } finally {
    await rm(temporaryRoot, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
