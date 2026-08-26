use crate::models::{now_epoch, ProcessInfo, ServiceIdentity, ServiceSnapshot, WorkspaceSnapshot};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    time::Instant,
};
use sysinfo::{Pid, System};

mod classification;
mod demo;
mod listeners;
mod presentation;
mod project;
mod spring;

use classification::classify;
pub(crate) use demo::demo_snapshot;
use listeners::{current_uid, listeners_from_lsof, listeners_from_ss, ListenerRecord};
use presentation::{
    browser_url, display_name, lowest_port, origin_for, redact_command_full, service_id,
    truncate_command,
};
use project::{detect_project, inferred_existing_path};
use spring::{refine_spring_listeners, resolve_spring_settings};

#[cfg(test)]
use crate::models::{Endpoint, ProjectInfo};
#[cfg(test)]
use classification::is_build_daemon;
#[cfg(test)]
use listeners::parse_endpoint;
#[cfg(test)]
use project::{is_excluded_project_root, looks_like_absolute_path, project_has_spring_evidence};
#[cfg(test)]
use spring::{parse_spring_yaml, spring_context_path, SpringSettings};
#[cfg(test)]
use std::{fs, path::PathBuf};

pub fn scan_workspace(
    demo: bool,
) -> Result<(WorkspaceSnapshot, HashMap<String, ServiceIdentity>), String> {
    if demo {
        let snapshot = demo_snapshot();
        let index = snapshot
            .services
            .iter()
            .filter_map(|service| {
                service.process.as_ref().map(|process| {
                    (
                        service.id.clone(),
                        ServiceIdentity {
                            pid: process.pid,
                            start_time: process.create_time,
                            uid: process.uid,
                            display_name: service.display_name.clone(),
                        },
                    )
                })
            })
            .collect();
        return Ok((snapshot, index));
    }

    let started = Instant::now();
    let current_uid = current_uid();
    let mut errors = Vec::new();
    let records = match listeners_from_lsof() {
        Ok(records) => records,
        Err(lsof_error) => match listeners_from_ss(current_uid) {
            Ok(records) => {
                errors.push(format!("lsof unavailable; using ss: {lsof_error}"));
                records
            }
            Err(ss_error) => {
                return Err(format!(
                    "Could not inspect TCP listeners. lsof: {lsof_error}; ss: {ss_error}"
                ));
            }
        },
    };
    let endpoint_count = records.len();
    let mut grouped: BTreeMap<u32, Vec<ListenerRecord>> = BTreeMap::new();
    for record in records {
        grouped.entry(record.pid).or_default().push(record);
    }

    let system = System::new_all();
    let own_pid = std::process::id();
    let mut services = Vec::new();
    let mut index = HashMap::new();

    for (pid, mut listeners) in grouped {
        listeners.sort_by_key(|record| (record.endpoint.port, record.endpoint.address.clone()));
        listeners.dedup_by(|left, right| left.endpoint == right.endpoint);
        let process = system.process(Pid::from_u32(pid));
        let process_name = process
            .map(|item| item.name().to_string_lossy().into_owned())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| listeners[0].process_name.clone());
        let args = process
            .map(|item| {
                item.cmd()
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let environment = process
            .map(|item| {
                item.environ()
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let executable = process
            .and_then(|item| item.exe())
            .map(|path| path.to_string_lossy().into_owned());
        let cwd = process
            .and_then(|item| item.cwd())
            .map(Path::to_path_buf)
            .or_else(|| inferred_existing_path(&args));
        let project = detect_project(cwd.as_deref(), &args);
        let observed_ports = listeners
            .iter()
            .map(|record| record.endpoint.port)
            .collect::<Vec<_>>();
        let classification = classify(
            &process_name,
            executable.as_deref(),
            &args,
            &observed_ports,
            project.as_ref(),
        );
        let uid = listeners.iter().find_map(|record| record.uid);

        if classification.relevance == "noise"
            || (uid.is_some() && current_uid.is_some() && uid != current_uid)
        {
            continue;
        }

        let spring_settings = if classification.tech == "spring" {
            let settings = resolve_spring_settings(
                &args,
                &environment,
                cwd.as_deref(),
                project.as_ref().map(|value| Path::new(&value.root_path)),
            );
            if !refine_spring_listeners(&mut listeners, &settings) {
                continue;
            }
            Some(settings)
        } else {
            None
        };
        let ports = listeners
            .iter()
            .map(|record| record.endpoint.port)
            .collect::<Vec<_>>();

        let start_time = process.map(|item| item.start_time()).unwrap_or(0);
        let id = service_id(pid, start_time, &ports);
        let launch_command = redact_command_full(&args, &process_name);
        let process_info = process.map(|item| ProcessInfo {
            pid,
            parent_pid: item.parent().map(|value| value.as_u32()),
            name: process_name.clone(),
            executable: executable.clone(),
            working_directory: cwd.as_ref().map(|path| path.to_string_lossy().into_owned()),
            command: truncate_command(&launch_command),
            launch_command: Some(launch_command),
            create_time: item.start_time(),
            uptime_seconds: item.run_time(),
            cpu_percent: Some(item.cpu_usage()),
            memory_bytes: Some(item.memory()),
            uid,
        });
        let endpoints = listeners
            .into_iter()
            .map(|record| record.endpoint)
            .collect::<Vec<_>>();
        let (origin_kind, origin_label) = origin_for(process, &system);
        let mut warnings = Vec::new();
        if process.is_none() {
            warnings.push("Process details were unavailable during this scan.".into());
        }
        if endpoints
            .iter()
            .any(|endpoint| endpoint.scope == "wildcard")
        {
            warnings.push("This listener is reachable on every network interface.".into());
        }
        let can_terminate = process.is_some()
            && pid > 1
            && pid != own_pid
            && (uid.is_none() || current_uid.is_none() || uid == current_uid)
            && classification.relevance == "dev";
        let display_name = display_name(&process_name, project.as_ref(), &classification.tech);
        let browser_url = browser_url(
            &classification.category,
            &endpoints,
            &args,
            spring_settings.as_ref(),
        );
        let service = ServiceSnapshot {
            id: id.clone(),
            display_name: display_name.clone(),
            tech: classification.tech,
            category: classification.category,
            relevance: classification.relevance,
            endpoints,
            process: process_info,
            project,
            status: if warnings.is_empty() {
                "healthy".into()
            } else {
                "limited".into()
            },
            warnings,
            origin_kind,
            origin_label,
            can_terminate,
            browser_url,
            active_profiles: spring_settings
                .as_ref()
                .map(|settings| settings.active_profiles.clone())
                .unwrap_or_default(),
        };
        if can_terminate {
            index.insert(
                id,
                ServiceIdentity {
                    pid,
                    start_time,
                    uid,
                    display_name,
                },
            );
        }
        services.push(service);
    }

    services.sort_by(|left, right| {
        let left_path = left
            .project
            .as_ref()
            .map(|item| item.root_path.as_str())
            .unwrap_or("~");
        let right_path = right
            .project
            .as_ref()
            .map(|item| item.root_path.as_str())
            .unwrap_or("~");
        left_path
            .cmp(right_path)
            .then_with(|| lowest_port(left).cmp(&lowest_port(right)))
            .then_with(|| {
                left.display_name
                    .to_lowercase()
                    .cmp(&right.display_name.to_lowercase())
            })
    });

    Ok((
        WorkspaceSnapshot {
            services,
            scanned_at: now_epoch(),
            scan_duration_ms: started.elapsed().as_millis(),
            endpoint_count,
            errors,
        },
        index,
    ))
}

#[cfg(test)]
mod tests;
