use std::str::FromStr;

#[derive(Debug, Default)]
pub(crate) struct CliOptions {
    pub(crate) demo: bool,
    pub(crate) auto_close_seconds: Option<f64>,
    pub(crate) show_help: bool,
    pub(crate) show_version: bool,
    pub(crate) control: Option<ControlCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ControlCommand {
    Help,
    List,
    Status {
        profile_id: String,
        task_name: String,
    },
    Logs {
        profile_id: String,
        task_name: String,
        lines: Option<u32>,
        max_bytes: Option<u32>,
    },
    Start {
        profile_id: String,
        task_name: String,
        timeout_seconds: u32,
    },
    Stop {
        profile_id: String,
        task_name: String,
        timeout_seconds: u32,
    },
    Restart {
        profile_id: String,
        task_name: String,
        timeout_seconds: u32,
    },
    Operation {
        operation_id: String,
    },
}

const DEFAULT_ACTION_TIMEOUT_SECONDS: u32 = 30;
const MAX_ACTION_TIMEOUT_SECONDS: u32 = 300;
const MAX_LOG_LINES: u32 = 1_000;
const MAX_LOG_BYTES: u32 = 65_536;

pub(crate) fn parse_cli() -> Result<CliOptions, String> {
    parse_cli_from(std::env::args().skip(1))
}

pub(crate) fn parse_cli_from<I, S>(arguments: I) -> Result<CliOptions, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut arguments = arguments.into_iter().map(Into::into).peekable();
    if arguments
        .peek()
        .is_some_and(|argument| argument == "control")
    {
        arguments.next();
        return Ok(CliOptions {
            control: Some(parse_control_command(arguments)?),
            ..CliOptions::default()
        });
    }

    let mut options = CliOptions::default();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--demo" => options.demo = true,
            "--help" | "-h" => options.show_help = true,
            "--version" | "-V" => options.show_version = true,
            "--auto-close-seconds" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| "--auto-close-seconds requires a value".to_string())?;
                options.auto_close_seconds = Some(parse_positive_seconds(&value)?);
            }
            value if value.starts_with("--auto-close-seconds=") => {
                options.auto_close_seconds = Some(parse_positive_seconds(
                    value.trim_start_matches("--auto-close-seconds="),
                )?);
            }
            value => return Err(format!("Unknown option: {value}")),
        }
    }
    Ok(options)
}

pub(crate) fn control_was_requested() -> bool {
    std::env::args().nth(1).as_deref() == Some("control")
}

fn parse_control_command<I>(mut arguments: I) -> Result<ControlCommand, String>
where
    I: Iterator<Item = String>,
{
    let command = arguments
        .next()
        .ok_or_else(|| "control requires a subcommand".to_string())?;
    if command == "--help" || command == "-h" || command == "help" {
        ensure_no_more(arguments)?;
        return Ok(ControlCommand::Help);
    }
    if command == "list" {
        ensure_no_more(arguments)?;
        return Ok(ControlCommand::List);
    }
    if command == "operation" {
        let operation_id = arguments
            .next()
            .ok_or_else(|| "control operation requires an operation ID".to_string())?;
        ensure_no_more(arguments)?;
        if operation_id.trim().is_empty() {
            return Err("The operation ID cannot be empty.".into());
        }
        if operation_id.len() > 128 {
            return Err("The operation ID cannot exceed 128 bytes.".into());
        }
        return Ok(ControlCommand::Operation { operation_id });
    }

    let mut profile_id = None;
    let mut task_name = None;
    let mut lines = None;
    let mut max_bytes = None;
    let mut timeout_seconds = DEFAULT_ACTION_TIMEOUT_SECONDS;
    let mut timeout_was_set = false;

    while let Some(flag) = arguments.next() {
        let value = match flag.as_str() {
            "--profile" => arguments
                .next()
                .ok_or_else(|| "--profile requires an ID".to_string())?,
            "--task" => arguments
                .next()
                .ok_or_else(|| "--task requires a name".to_string())?,
            "--lines" if command == "logs" => {
                if lines.is_some() {
                    return Err("--lines may be provided only once.".into());
                }
                let value = arguments
                    .next()
                    .ok_or_else(|| "--lines requires a number".to_string())?;
                lines = Some(parse_bounded_integer(&value, "lines", 1, MAX_LOG_LINES)?);
                continue;
            }
            "--max-bytes" if command == "logs" => {
                if max_bytes.is_some() {
                    return Err("--max-bytes may be provided only once.".into());
                }
                let value = arguments
                    .next()
                    .ok_or_else(|| "--max-bytes requires a number".to_string())?;
                max_bytes = Some(parse_bounded_integer(
                    &value,
                    "max bytes",
                    1,
                    MAX_LOG_BYTES,
                )?);
                continue;
            }
            "--timeout-seconds" if matches!(command.as_str(), "start" | "stop" | "restart") => {
                if timeout_was_set {
                    return Err("--timeout-seconds may be provided only once.".into());
                }
                let value = arguments
                    .next()
                    .ok_or_else(|| "--timeout-seconds requires a number".to_string())?;
                timeout_seconds = parse_bounded_integer(
                    &value,
                    "timeout seconds",
                    1,
                    MAX_ACTION_TIMEOUT_SECONDS,
                )?;
                timeout_was_set = true;
                continue;
            }
            _ => return Err("Unknown or unsupported control option.".into()),
        };
        if value.trim().is_empty() {
            return Err(format!("{flag} cannot be empty."));
        }
        if flag == "--profile" {
            if profile_id.is_some() {
                return Err("--profile may be provided only once.".into());
            }
            profile_id = Some(value);
        } else {
            if task_name.is_some() {
                return Err("--task may be provided only once.".into());
            }
            task_name = Some(value);
        }
    }

    match command.as_str() {
        "status" | "logs" | "start" | "stop" | "restart" => {
            let profile_id =
                profile_id.ok_or_else(|| format!("control {command} requires --profile <ID>"))?;
            let task_name =
                task_name.ok_or_else(|| format!("control {command} requires --task <NAME>"))?;
            Ok(match command.as_str() {
                "status" => ControlCommand::Status {
                    profile_id,
                    task_name,
                },
                "logs" => ControlCommand::Logs {
                    profile_id,
                    task_name,
                    lines,
                    max_bytes,
                },
                "start" => ControlCommand::Start {
                    profile_id,
                    task_name,
                    timeout_seconds,
                },
                "stop" => ControlCommand::Stop {
                    profile_id,
                    task_name,
                    timeout_seconds,
                },
                _ => ControlCommand::Restart {
                    profile_id,
                    task_name,
                    timeout_seconds,
                },
            })
        }
        _ => Err("Unknown control subcommand.".into()),
    }
}

fn ensure_no_more(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    if arguments.next().is_some() {
        Err("Unexpected trailing argument.".into())
    } else {
        Ok(())
    }
}

fn parse_bounded_integer(
    value: &str,
    description: &str,
    minimum: u32,
    maximum: u32,
) -> Result<u32, String> {
    let parsed =
        u32::from_str(value).map_err(|_| format!("Invalid {description}: expected an integer."))?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(format!(
            "Invalid {description}: expected a value from {minimum} to {maximum}."
        ));
    }
    Ok(parsed)
}

fn parse_positive_seconds(value: &str) -> Result<f64, String> {
    let seconds = value
        .parse::<f64>()
        .map_err(|_| format!("Invalid number of seconds: {value}"))?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err("Auto-close seconds must be greater than zero.".into());
    }
    Ok(seconds)
}

pub(crate) fn print_help() {
    println!(
        "Cutting Board {}\n\nUSAGE:\n    cutting-board [OPTIONS]\n    cutting-board control <COMMAND>\n\nOPTIONS:\n    --demo                       Show deterministic sample services\n    --auto-close-seconds <N>     Close automatically after N seconds\n    -h, --help                   Print help\n    -V, --version                Print version\n\nCONTROL COMMANDS:\n    list\n    status --profile <ID> --task <NAME>\n    logs --profile <ID> --task <NAME> [--lines N] [--max-bytes N]\n    start|stop|restart --profile <ID> --task <NAME> [--timeout-seconds N]\n    operation <ID>",
        env!("CARGO_PKG_VERSION")
    );
}

pub(crate) fn print_control_help() {
    println!(
        "Cutting Board control\n\nUSAGE:\n    cutting-board control list\n    cutting-board control status --profile <ID> --task <NAME>\n    cutting-board control logs --profile <ID> --task <NAME> [--lines N] [--max-bytes N]\n    cutting-board control start|stop|restart --profile <ID> --task <NAME> [--timeout-seconds N]\n    cutting-board control operation <ID>\n\nAction waits default to 30 seconds and accept values from 1 to 300. Log requests accept up to 1000 lines and 65536 bytes."
    );
}

#[cfg(test)]
mod tests {
    use super::{parse_cli_from, ControlCommand};

    #[test]
    fn parses_existing_app_options() {
        let options = parse_cli_from(["--demo", "--auto-close-seconds=0.5"]).unwrap();
        assert!(options.demo);
        assert_eq!(options.auto_close_seconds, Some(0.5));
        assert!(options.control.is_none());
    }

    #[test]
    fn parses_control_list_and_task_commands() {
        assert!(matches!(
            parse_cli_from(["control", "list"]).unwrap().control,
            Some(ControlCommand::List)
        ));
        assert_eq!(
            parse_cli_from([
                "control",
                "restart",
                "--profile",
                "p1",
                "--task",
                "backend",
                "--timeout-seconds",
                "90"
            ])
            .unwrap()
            .control,
            Some(ControlCommand::Restart {
                profile_id: "p1".into(),
                task_name: "backend".into(),
                timeout_seconds: 90,
            })
        );
    }

    #[test]
    fn parses_bounded_log_options_and_operation_poll() {
        assert_eq!(
            parse_cli_from([
                "control",
                "logs",
                "--profile",
                "p1",
                "--task",
                "backend",
                "--lines",
                "1000",
                "--max-bytes",
                "65536"
            ])
            .unwrap()
            .control,
            Some(ControlCommand::Logs {
                profile_id: "p1".into(),
                task_name: "backend".into(),
                lines: Some(1000),
                max_bytes: Some(65536),
            })
        );
        assert_eq!(
            parse_cli_from(["control", "operation", "op-1"])
                .unwrap()
                .control,
            Some(ControlCommand::Operation {
                operation_id: "op-1".into()
            })
        );
    }

    #[test]
    fn rejects_invalid_or_unbounded_control_arguments() {
        for arguments in [
            vec!["control", "start", "--profile", "p1"],
            vec![
                "control",
                "restart",
                "--profile",
                "p1",
                "--task",
                "x",
                "--timeout-seconds",
                "301",
            ],
            vec![
                "control",
                "logs",
                "--profile",
                "p1",
                "--task",
                "x",
                "--lines",
                "1001",
            ],
            vec![
                "control",
                "logs",
                "--profile",
                "p1",
                "--task",
                "x",
                "--max-bytes",
                "65537",
            ],
            vec![
                "control",
                "restart",
                "--profile",
                "p1",
                "--task",
                "x",
                "--socket",
                "/tmp/evil",
            ],
        ] {
            assert!(parse_cli_from(arguments).is_err());
        }
    }

    #[test]
    fn preserves_auto_close_validation() {
        assert!(parse_cli_from(["--auto-close-seconds", "0"]).is_err());
        assert!(parse_cli_from(["--unknown"]).is_err());
    }
}
