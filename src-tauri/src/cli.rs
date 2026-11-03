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
        current_run: bool,
        prepare: bool,
    },
    Wait {
        profile_id: String,
        task_name: String,
        log_pattern: Option<String>,
        timeout_seconds: u32,
    },
    Show {
        profile_id: String,
        task_name: String,
    },
    Update {
        profile_id: String,
        task_name: String,
        command: Option<String>,
        cwd: Option<String>,
        expected_port: Option<u16>,
        clear_expected_port: bool,
        clear_prepare: bool,
        timeout_seconds: u32,
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
const DEFAULT_WAIT_TIMEOUT_SECONDS: u32 = 120;
const MAX_ACTION_TIMEOUT_SECONDS: u32 = 300;
const MAX_LOG_LINES: u32 = 1_000;
const MAX_LOG_BYTES: u32 = 65_536;
const CONTROL_HELP: &str = include_str!("control_help.txt");

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
            "help" | "--help" | "-h" => {
                options.show_help = true;
                break;
            }
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

fn is_help_flag(argument: &str) -> bool {
    argument == "--help" || argument == "-h"
}

fn parse_control_command<I>(arguments: I) -> Result<ControlCommand, String>
where
    I: Iterator<Item = String>,
{
    let mut arguments = arguments.peekable();
    let command = arguments
        .next()
        .ok_or_else(|| "control requires a subcommand".to_string())?;
    if command == "help"
        || is_help_flag(&command)
        || arguments.peek().is_some_and(|next| is_help_flag(next))
    {
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
    let mut current_run = false;
    let mut prepare = false;
    let mut log_pattern = None;
    let mut new_command = None;
    let mut new_cwd = None;
    let mut expected_port = None;
    let mut clear_expected_port = false;
    let mut clear_prepare = false;
    let mut timeout_seconds = if command == "wait" {
        DEFAULT_WAIT_TIMEOUT_SECONDS
    } else {
        DEFAULT_ACTION_TIMEOUT_SECONDS
    };
    let mut timeout_was_set = false;

    while let Some(flag) = arguments.next() {
        if is_help_flag(&flag) {
            return Ok(ControlCommand::Help);
        }
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
            "--current-run" if command == "logs" => {
                current_run = true;
                continue;
            }
            "--prepare" if command == "logs" => {
                prepare = true;
                continue;
            }
            "--log-pattern" if command == "wait" => {
                set_text_option(&mut log_pattern, "--log-pattern", arguments.next())?;
                continue;
            }
            "--command" if command == "update" => {
                set_text_option(&mut new_command, "--command", arguments.next())?;
                continue;
            }
            "--cwd" if command == "update" => {
                set_text_option(&mut new_cwd, "--cwd", arguments.next())?;
                continue;
            }
            "--expected-port" if command == "update" => {
                if expected_port.is_some() {
                    return Err("--expected-port may be provided only once.".into());
                }
                let value = arguments
                    .next()
                    .ok_or_else(|| "--expected-port requires a number".to_string())?;
                expected_port =
                    Some(
                        parse_bounded_integer(&value, "expected port", 1, u32::from(u16::MAX))?
                            as u16,
                    );
                continue;
            }
            "--clear-expected-port" if command == "update" => {
                clear_expected_port = true;
                continue;
            }
            "--clear-prepare" if command == "update" => {
                clear_prepare = true;
                continue;
            }
            "--timeout-seconds"
                if matches!(
                    command.as_str(),
                    "start" | "stop" | "restart" | "wait" | "update"
                ) =>
            {
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
        "status" | "logs" | "start" | "stop" | "restart" | "wait" | "show" | "update" => {
            let profile_id =
                profile_id.ok_or_else(|| format!("control {command} requires --profile <ID>"))?;
            let task_name =
                task_name.ok_or_else(|| format!("control {command} requires --task <NAME>"))?;
            Ok(match command.as_str() {
                "status" => ControlCommand::Status {
                    profile_id,
                    task_name,
                },
                "logs" if current_run && prepare => {
                    return Err("--current-run and --prepare cannot be combined.".into())
                }
                "logs" => ControlCommand::Logs {
                    profile_id,
                    task_name,
                    lines,
                    max_bytes,
                    current_run,
                    prepare,
                },
                "wait" => ControlCommand::Wait {
                    profile_id,
                    task_name,
                    log_pattern,
                    timeout_seconds,
                },
                "show" => ControlCommand::Show {
                    profile_id,
                    task_name,
                },
                "update" => {
                    if expected_port.is_some() && clear_expected_port {
                        return Err(
                            "--expected-port and --clear-expected-port cannot be combined.".into(),
                        );
                    }
                    if new_command.is_none()
                        && new_cwd.is_none()
                        && expected_port.is_none()
                        && !clear_expected_port
                        && !clear_prepare
                    {
                        return Err("control update requires at least one change.".into());
                    }
                    ControlCommand::Update {
                        profile_id,
                        task_name,
                        command: new_command,
                        cwd: new_cwd,
                        expected_port,
                        clear_expected_port,
                        clear_prepare,
                        timeout_seconds,
                    }
                }
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

fn set_text_option(
    target: &mut Option<String>,
    flag: &str,
    value: Option<String>,
) -> Result<(), String> {
    if target.is_some() {
        return Err(format!("{flag} may be provided only once."));
    }
    let value = value.ok_or_else(|| format!("{flag} requires a value"))?;
    if value.trim().is_empty() {
        return Err(format!("{flag} cannot be empty."));
    }
    *target = Some(value);
    Ok(())
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
    print!(
        "Cutting Board {}\n\nUSAGE:\n    cutting-board [OPTIONS]\n    cutting-board control <COMMAND>\n    cutting-board help\n\nOPTIONS:\n    --demo                       Show deterministic sample services\n    --auto-close-seconds <N>     Close automatically after N seconds\n    -h, --help                   Print help\n    -V, --version                Print version\n\n{}",
        env!("CARGO_PKG_VERSION"),
        CONTROL_HELP
    );
}

pub(crate) fn print_control_help() {
    print!(
        "Cutting Board {}\n\n{}",
        env!("CARGO_PKG_VERSION"),
        CONTROL_HELP
    );
}

#[cfg(test)]
mod tests {
    use super::{parse_cli_from, ControlCommand, CONTROL_HELP};

    #[test]
    fn every_help_form_prints_help() {
        assert!(parse_cli_from(["help"]).unwrap().show_help);
        assert!(parse_cli_from(["--help"]).unwrap().show_help);
        for arguments in [
            vec!["control", "help"],
            vec!["control", "--help"],
            vec!["control", "wait", "--help"],
            vec!["control", "list", "-h"],
            vec!["control", "operation", "--help"],
            vec!["control", "logs", "--profile", "p1", "--help"],
        ] {
            assert_eq!(
                parse_cli_from(arguments.clone()).unwrap().control,
                Some(ControlCommand::Help),
                "{arguments:?}"
            );
        }
        // A flag's value is not a request for help.
        assert!(matches!(
            parse_cli_from([
                "control",
                "update",
                "--profile",
                "p1",
                "--task",
                "api",
                "--command",
                "--help",
            ])
            .unwrap()
            .control,
            Some(ControlCommand::Update { .. })
        ));
    }

    #[test]
    fn help_documents_every_command_and_flag() {
        for expected in [
            "control list",
            "control status",
            "control start|stop|restart",
            "control wait",
            "control logs",
            "control show",
            "control update",
            "control operation",
            "--lines",
            "--max-bytes",
            "--current-run",
            "--prepare",
            "--log-pattern",
            "--timeout-seconds",
            "--command",
            "--cwd",
            "--expected-port",
            "--clear-expected-port",
            "--clear-prepare",
        ] {
            assert!(CONTROL_HELP.contains(expected), "{expected}");
        }
    }

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
                current_run: false,
                prepare: false,
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
    fn parses_wait_with_its_own_default_timeout_and_current_run_logs() {
        let task = ["--profile", "p1", "--task", "api"];
        let parse = |subcommand: &str, extra: &[&'static str]| {
            let mut arguments = vec!["control", subcommand];
            arguments.extend(task);
            arguments.extend(extra);
            parse_cli_from(arguments).map(|options| options.control.unwrap())
        };
        let wait = |extra: &[&'static str]| parse("wait", extra);
        assert_eq!(
            wait(&[]).unwrap(),
            ControlCommand::Wait {
                profile_id: "p1".into(),
                task_name: "api".into(),
                log_pattern: None,
                timeout_seconds: 120,
            }
        );
        assert_eq!(
            wait(&["--log-pattern", "Started App", "--timeout-seconds", "5"]).unwrap(),
            ControlCommand::Wait {
                profile_id: "p1".into(),
                task_name: "api".into(),
                log_pattern: Some("Started App".into()),
                timeout_seconds: 5,
            }
        );
        assert!(wait(&["--timeout-seconds", "0"]).is_err());
        assert!(wait(&["--timeout-seconds", "301"]).is_err());
        assert!(wait(&["--log-pattern", " "]).is_err());
        assert!(wait(&["--current-run"]).is_err());
        assert!(matches!(
            parse("logs", &["--current-run"]).unwrap(),
            ControlCommand::Logs {
                current_run: true,
                ..
            }
        ));
        assert!(matches!(
            parse("logs", &["--prepare"]).unwrap(),
            ControlCommand::Logs { prepare: true, .. }
        ));
        assert!(parse("logs", &["--prepare", "--current-run"]).is_err());
        assert!(parse("status", &["--prepare"]).is_err());
        assert!(parse("logs", &["--log-pattern", "x"]).is_err());
        assert!(parse("status", &["--current-run"]).is_err());
        assert_eq!(
            parse("show", &[]).unwrap(),
            ControlCommand::Show {
                profile_id: "p1".into(),
                task_name: "api".into(),
            }
        );
        assert!(parse("show", &["--timeout-seconds", "5"]).is_err());
    }

    #[test]
    fn parses_update_and_requires_a_coherent_change() {
        let parse = |extra: &[&str]| {
            let mut arguments = vec!["control", "update", "--profile", "p1", "--task", "api"];
            arguments.extend(extra.iter().copied());
            parse_cli_from(arguments.into_iter().map(str::to_owned))
                .map(|options| options.control.unwrap())
        };
        assert_eq!(
            parse(&[
                "--command",
                "--looks-like-a-flag",
                "--cwd",
                "api",
                "--expected-port",
                "65535",
                "--clear-prepare",
                "--timeout-seconds",
                "9",
            ])
            .unwrap(),
            ControlCommand::Update {
                profile_id: "p1".into(),
                task_name: "api".into(),
                command: Some("--looks-like-a-flag".into()),
                cwd: Some("api".into()),
                expected_port: Some(65535),
                clear_expected_port: false,
                clear_prepare: true,
                timeout_seconds: 9,
            }
        );
        assert!(matches!(
            parse(&["--clear-expected-port"]).unwrap(),
            ControlCommand::Update {
                clear_expected_port: true,
                timeout_seconds: 30,
                ..
            }
        ));
        for invalid in [
            &[][..],
            &["--expected-port", "0"],
            &["--expected-port", "65536"],
            &["--expected-port", "1", "--clear-expected-port"],
            &["--command", "  "],
            &["--cwd", ""],
            &["--command", "a", "--command", "b"],
        ] {
            assert!(parse(invalid).is_err(), "{invalid:?}");
        }
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
