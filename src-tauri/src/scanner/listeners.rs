use crate::models::Endpoint;
use std::{net::IpAddr, process::Command, str::FromStr};

#[derive(Debug, Clone)]
pub(crate) struct ListenerRecord {
    pub(crate) pid: u32,
    pub(crate) uid: Option<u32>,
    pub(crate) process_name: String,
    pub(crate) endpoint: Endpoint,
}

pub(crate) fn listeners_from_lsof() -> Result<Vec<ListenerRecord>, String> {
    let output = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-FpcuPn"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() && output.stdout.is_empty() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut pid = None;
    let mut uid = None;
    let mut process_name = String::new();
    let mut protocol = String::new();
    let mut records = Vec::new();
    for line in text.lines() {
        let Some(field) = line.get(..1) else {
            continue;
        };
        let value = &line[1..];
        match field {
            "p" => {
                pid = value.parse::<u32>().ok();
                uid = None;
                process_name.clear();
                protocol.clear();
            }
            "c" => process_name = value.to_string(),
            "u" => uid = value.parse::<u32>().ok(),
            "P" => protocol = value.to_string(),
            "n" if protocol.eq_ignore_ascii_case("TCP") || protocol.is_empty() => {
                if let (Some(pid), Some(endpoint)) = (pid, parse_endpoint(value)) {
                    records.push(ListenerRecord {
                        pid,
                        uid,
                        process_name: process_name.clone(),
                        endpoint,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(records)
}

pub(crate) fn listeners_from_ss(uid: Option<u32>) -> Result<Vec<ListenerRecord>, String> {
    let output = Command::new("ss")
        .args(["-H", "-ltnp"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut records = Vec::new();
    for line in text.lines() {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 4 {
            continue;
        }
        let Some(endpoint) = parse_endpoint(columns[3]) else {
            continue;
        };
        let Some(pid) = parse_pid_from_ss(line) else {
            continue;
        };
        let process_name = parse_name_from_ss(line).unwrap_or_else(|| "process".into());
        records.push(ListenerRecord {
            pid,
            uid,
            process_name,
            endpoint,
        });
    }
    Ok(records)
}

fn parse_pid_from_ss(line: &str) -> Option<u32> {
    let start = line.find("pid=")? + 4;
    let digits = line[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits.parse().ok()
}

fn parse_name_from_ss(line: &str) -> Option<String> {
    let marker = "((\"";
    let start = line.find(marker)? + marker.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

pub(crate) fn parse_endpoint(value: &str) -> Option<Endpoint> {
    let value = value.trim().trim_start_matches("TCP ");
    let local = value.split("->").next()?.trim();
    let (address, port_text) = if let Some(rest) = local.strip_prefix('[') {
        let end = rest.find(']')?;
        let address = &rest[..end];
        let port = rest[end + 1..].strip_prefix(':')?;
        (address, port)
    } else {
        local.rsplit_once(':')?
    };
    let port = port_text.trim_matches('*').parse::<u16>().ok()?;
    let address = match address.trim() {
        "" | "*" => "0.0.0.0".to_string(),
        value => value.trim_matches(['[', ']']).to_string(),
    };
    let family = if address.contains(':') {
        "IPv6"
    } else {
        "IPv4"
    };
    let scope = endpoint_scope(&address);
    Some(Endpoint {
        family: family.into(),
        address,
        port,
        scope,
        protocol: "TCP".into(),
    })
}

fn endpoint_scope(address: &str) -> String {
    if matches!(address, "0.0.0.0" | "::" | "*") {
        return "wildcard".into();
    }
    match IpAddr::from_str(address) {
        Ok(ip) if ip.is_loopback() => "loopback".into(),
        Ok(ip) if ip.is_unspecified() => "wildcard".into(),
        Ok(_) => "lan".into(),
        Err(_) if address.eq_ignore_ascii_case("localhost") => "loopback".into(),
        Err(_) => "lan".into(),
    }
}

pub(crate) fn current_uid() -> Option<u32> {
    let output = Command::new("id").arg("-u").output().ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}
