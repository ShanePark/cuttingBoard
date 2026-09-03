use std::{
    collections::VecDeque,
    io::Read,
    process::{Child, ExitStatus},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use tauri::AppHandle;

use super::{emit_update_progress, UpdateProgress};

const LOG_INTERVAL: Duration = Duration::from_millis(500);
const LOG_MAX_CHARS: usize = 180;
const DIAGNOSTIC_TAIL_LINES: usize = 24;

#[derive(Debug)]
enum Message {
    Line(String),
}

#[derive(Debug)]
struct Collector {
    diagnostic_tail: VecDeque<String>,
    latest_detail: Option<String>,
    emitted_detail: Option<String>,
    last_detail_emitted_at: Option<Instant>,
}

impl Default for Collector {
    fn default() -> Self {
        Self {
            diagnostic_tail: VecDeque::with_capacity(DIAGNOSTIC_TAIL_LINES),
            latest_detail: None,
            emitted_detail: None,
            last_detail_emitted_at: None,
        }
    }
}

impl Collector {
    fn record_line(&mut self, raw_line: String, app: &AppHandle) {
        let Some(clean_line) = clean_log(&raw_line) else {
            return;
        };

        if self.diagnostic_tail.len() == DIAGNOSTIC_TAIL_LINES {
            self.diagnostic_tail.pop_front();
        }
        self.diagnostic_tail.push_back(clean_line.clone());

        let detail = truncate_log(&clean_line);
        self.latest_detail = Some(detail.clone());
        let should_emit = self
            .last_detail_emitted_at
            .map_or(true, |last| last.elapsed() >= LOG_INTERVAL);
        if should_emit {
            emit_update_progress(app, UpdateProgress::building_with_detail(detail.clone()));
            self.last_detail_emitted_at = Some(Instant::now());
            self.emitted_detail = Some(detail);
        }
    }

    fn emit_latest_detail(&mut self, app: &AppHandle) {
        let Some(detail) = self.latest_detail.as_ref() else {
            return;
        };
        if self.emitted_detail.as_ref() == Some(detail) {
            return;
        }
        emit_update_progress(app, UpdateProgress::building_with_detail(detail.clone()));
        self.emitted_detail = Some(detail.clone());
        self.last_detail_emitted_at = Some(Instant::now());
    }

    fn emit_due_detail(&mut self, app: &AppHandle) {
        let now = Instant::now();
        if detail_is_due(
            self.latest_detail.as_deref(),
            self.emitted_detail.as_deref(),
            self.last_detail_emitted_at,
            now,
        ) {
            self.emit_latest_detail(app);
        }
    }

    fn diagnostics(&self) -> String {
        let lines = self.diagnostic_tail.iter().cloned().collect::<Vec<_>>();
        command_diagnostics(&lines)
    }
}

#[derive(Debug)]
pub(super) struct BuildOutputResult {
    pub(super) status: ExitStatus,
    pub(super) diagnostics: String,
    pub(super) reader_error: Option<String>,
}

pub(super) fn collect(
    app: &AppHandle,
    mut child: Child,
) -> std::result::Result<BuildOutputResult, String> {
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_child(&mut child);
            return Err("The release build did not expose a stdout stream.".into());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            terminate_child(&mut child);
            return Err("The release build did not expose a stderr stream.".into());
        }
    };

    let (sender, receiver) = mpsc::channel();
    let stdout_reader = match spawn_reader(stdout, "stdout", sender.clone()) {
        Ok(reader) => reader,
        Err(error) => {
            terminate_child(&mut child);
            return Err(error);
        }
    };
    let stderr_reader = match spawn_reader(stderr, "stderr", sender.clone()) {
        Ok(reader) => reader,
        Err(error) => {
            terminate_child(&mut child);
            let _ = join_reader(stdout_reader, "stdout");
            return Err(error);
        }
    };
    drop(sender);

    let mut collector = Collector::default();
    let status = loop {
        drain(&receiver, &mut collector, app);
        collector.emit_due_detail(app);
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(Message::Line(line)) => collector.record_line(line, app),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // Both readers can finish before a shell descendant has
                    // exited. Keep waiting while avoiding a tight loop.
                    thread::sleep(Duration::from_millis(50));
                }
            },
            Err(error) => {
                terminate_child(&mut child);
                let _ = join_reader(stdout_reader, "stdout");
                let _ = join_reader(stderr_reader, "stderr");
                return Err(format!("Could not wait for the release build: {error}"));
            }
        }
    };

    let stdout_error = join_reader(stdout_reader, "stdout").err();
    let stderr_error = join_reader(stderr_reader, "stderr").err();
    drain(&receiver, &mut collector, app);
    collector.emit_latest_detail(app);

    std::result::Result::Ok(BuildOutputResult {
        status,
        diagnostics: collector.diagnostics(),
        reader_error: stdout_error.or(stderr_error),
    })
}

fn drain(receiver: &Receiver<Message>, collector: &mut Collector, app: &AppHandle) {
    while let Ok(Message::Line(line)) = receiver.try_recv() {
        collector.record_line(line, app);
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    reader: R,
    stream_name: &'static str,
    sender: Sender<Message>,
) -> std::result::Result<thread::JoinHandle<std::result::Result<(), String>>, String> {
    thread::Builder::new()
        .name(format!("cutting-board-update-{stream_name}"))
        .spawn(move || read_stream(reader, sender))
        .map_err(|error| format!("Could not start the {stream_name} build output reader: {error}"))
}

fn join_reader(
    reader: thread::JoinHandle<std::result::Result<(), String>>,
    stream_name: &'static str,
) -> std::result::Result<(), String> {
    match reader.join() {
        Ok(result) => {
            result.map_err(|error| format!("The {stream_name} build output reader failed: {error}"))
        }
        Err(_) => Err(format!("The {stream_name} build output reader panicked.")),
    }
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn read_stream<R: Read>(mut reader: R, sender: Sender<Message>) -> std::result::Result<(), String> {
    let mut buffer = [0_u8; 8 * 1024];
    let mut line = Vec::new();
    let mut pending_carriage_return = false;

    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("Could not read release build output: {error}"))?;
        if read == 0 {
            break;
        }

        for byte in &buffer[..read] {
            if pending_carriage_return {
                pending_carriage_return = false;
                let is_line_feed = *byte == b'\n';
                if sender
                    .send(Message::Line(String::from_utf8_lossy(&line).into_owned()))
                    .is_err()
                {
                    return Ok(());
                }
                line.clear();
                if is_line_feed {
                    continue;
                }
            }

            match *byte {
                b'\r' => pending_carriage_return = true,
                b'\n' => {
                    if sender
                        .send(Message::Line(String::from_utf8_lossy(&line).into_owned()))
                        .is_err()
                    {
                        return Ok(());
                    }
                    line.clear();
                }
                byte => line.push(byte),
            }
        }
    }

    if pending_carriage_return || !line.is_empty() {
        let _ = sender.send(Message::Line(String::from_utf8_lossy(&line).into_owned()));
    }
    Ok(())
}

fn clean_log(raw_line: &str) -> Option<String> {
    let mut cleaned = String::with_capacity(raw_line.len());
    let mut characters = raw_line.chars();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' {
            match characters.next() {
                Some('[') => {
                    for sequence_character in characters.by_ref() {
                        if ('@'..='~').contains(&sequence_character) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    let mut escaped = false;
                    for sequence_character in characters.by_ref() {
                        if sequence_character == '\u{7}' {
                            break;
                        }
                        if escaped && sequence_character == '\\' {
                            break;
                        }
                        escaped = sequence_character == '\u{1b}';
                    }
                }
                Some(_) | None => {}
            }
        } else if character == '\t' {
            cleaned.push(' ');
        } else if character.is_control() {
            cleaned.push(' ');
        } else {
            cleaned.push(character);
        }
    }

    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    (!cleaned.is_empty()).then_some(cleaned)
}

fn truncate_log(value: &str) -> String {
    let mut characters = value.chars();
    let truncated = characters.by_ref().take(LOG_MAX_CHARS).collect::<String>();
    if characters.next().is_some() {
        truncated
            .chars()
            .take(LOG_MAX_CHARS.saturating_sub(1))
            .collect::<String>()
            + "…"
    } else {
        truncated
    }
}

fn detail_is_due(
    latest_detail: Option<&str>,
    emitted_detail: Option<&str>,
    last_emitted_at: Option<Instant>,
    now: Instant,
) -> bool {
    let Some(latest_detail) = latest_detail else {
        return false;
    };
    if emitted_detail == Some(latest_detail) {
        return false;
    }
    last_emitted_at.map_or(true, |last| now >= last + LOG_INTERVAL)
}

fn command_diagnostics(lines: &[String]) -> String {
    let diagnostics = lines.join(" ").trim().to_string();
    const MAX_DIAGNOSTICS: usize = 2_000;
    if diagnostics.len() > MAX_DIAGNOSTICS {
        let start = diagnostics
            .char_indices()
            .rev()
            .find_map(|(index, _)| (index <= diagnostics.len() - MAX_DIAGNOSTICS).then_some(index))
            .unwrap_or(0);
        diagnostics[start..].to_string()
    } else {
        diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleaning_removes_terminal_sequences_and_blank_lines() {
        assert_eq!(
            clean_log("\u{1b}[32mCompiling\u{1b}[0m\tcutting-board  "),
            Some("Compiling cutting-board".into())
        );
        assert_eq!(clean_log("\u{1b}]0;build\u{7}"), None);
    }

    #[test]
    fn display_is_truncated_at_the_unicode_boundary() {
        let input = "한".repeat(LOG_MAX_CHARS + 10);
        let output = truncate_log(&input);
        assert_eq!(output.chars().count(), LOG_MAX_CHARS);
        assert_eq!(output.chars().last(), Some('…'));
    }

    #[test]
    fn reader_handles_newline_and_carriage_return_progress() {
        let (sender, receiver) = mpsc::channel();
        read_stream(
            std::io::Cursor::new(b"first\r\nsecond\rthird\n\n".to_vec()),
            sender,
        )
        .unwrap();
        let lines = receiver
            .into_iter()
            .map(|message| match message {
                Message::Line(line) => line,
            })
            .collect::<Vec<_>>();
        assert_eq!(lines, ["first", "second", "third", ""]);
    }

    #[test]
    fn diagnostics_keep_only_the_recent_tail() {
        let lines = (0..(DIAGNOSTIC_TAIL_LINES + 3))
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>();
        let mut collector = Collector::default();
        for line in lines {
            if collector.diagnostic_tail.len() == DIAGNOSTIC_TAIL_LINES {
                collector.diagnostic_tail.pop_front();
            }
            collector.diagnostic_tail.push_back(line);
        }
        let diagnostics = collector.diagnostics();
        assert!(!diagnostics.contains("line-0"));
        assert!(diagnostics.contains("line-3"));
        assert!(diagnostics.contains("line-26"));
    }

    #[test]
    fn pending_detail_becomes_due_after_the_throttle_interval() {
        let started_at = Instant::now();
        assert!(!detail_is_due(
            Some("latest"),
            Some("first"),
            Some(started_at),
            started_at + LOG_INTERVAL - Duration::from_millis(1),
        ));
        assert!(detail_is_due(
            Some("latest"),
            Some("first"),
            Some(started_at),
            started_at + LOG_INTERVAL,
        ));
        assert!(!detail_is_due(
            Some("first"),
            Some("first"),
            Some(started_at),
            started_at + LOG_INTERVAL,
        ));
    }
}
