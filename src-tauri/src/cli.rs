#[derive(Debug, Default)]
pub(crate) struct CliOptions {
    pub(crate) demo: bool,
    pub(crate) auto_close_seconds: Option<f64>,
    pub(crate) show_help: bool,
    pub(crate) show_version: bool,
}

pub(crate) fn parse_cli() -> Result<CliOptions, String> {
    let mut options = CliOptions::default();
    let mut arguments = std::env::args().skip(1).peekable();
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
        "Cutting Board {}\n\nUSAGE:\n    cutting-board [OPTIONS]\n\nOPTIONS:\n    --demo                       Show deterministic sample services\n    --auto-close-seconds <N>     Close automatically after N seconds\n    -h, --help                   Print help\n    -V, --version                Print version",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::parse_positive_seconds;

    #[test]
    fn parses_auto_close_option() {
        assert_eq!(parse_positive_seconds("0.5").unwrap(), 0.5);
        assert!(parse_positive_seconds("0").is_err());
    }
}
