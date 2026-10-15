use std::{
    collections::HashMap,
    io::{ErrorKind, Read},
    sync::{Mutex, MutexGuard, PoisonError},
};

/// Bytes of `docker compose down` output kept per project; older lines are dropped first.
const LOG_LIMIT: usize = 64 * 1024;

/// The output of the latest `docker compose down` run for each project, readable while it runs.
#[derive(Debug, Default)]
pub struct ComposeDownLogs(Mutex<HashMap<String, String>>);

impl ComposeDownLogs {
    pub(super) fn begin(&self, project: &str) {
        self.entries().insert(project.to_owned(), String::new());
    }

    pub(super) fn append(&self, project: &str, line: &str) {
        append_line(self.entries().entry(project.to_owned()).or_default(), line);
    }

    pub(super) fn read(&self, project: &str) -> String {
        self.entries().get(project).cloned().unwrap_or_default()
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<String, String>> {
        // The map holds only display text, so a panic while it was held cannot break an invariant.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn append_line(log: &mut String, line: &str) {
    log.push_str(line);
    log.push('\n');
    if log.len() <= LOG_LIMIT {
        return;
    }
    let excess = log.len() - LOG_LIMIT;
    // A newline byte is always a character boundary, so cutting after one keeps valid UTF-8.
    let cut = log.as_bytes()[excess..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(log.len(), |offset| excess + offset + 1);
    log.drain(..cut);
}

/// Reads a process stream and reports each non-empty line, splitting on `\n` and `\r`.
pub(super) fn read_lines(mut reader: impl Read, mut on_line: impl FnMut(&str)) {
    let mut pending = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut flush = |pending: &mut Vec<u8>| {
        if !pending.is_empty() {
            on_line(&String::from_utf8_lossy(pending));
            pending.clear();
        }
    };
    loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        for &byte in &chunk[..read] {
            if byte == b'\n' || byte == b'\r' {
                flush(&mut pending);
            } else {
                pending.push(byte);
            }
        }
    }
    flush(&mut pending);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn splits_lines_on_newlines_and_carriage_returns() {
        let mut lines = Vec::new();
        read_lines(
            Cursor::new("Container a  Stopping\r\nContainer a  Stopped\rNetwork n  Removed"),
            |line| lines.push(line.to_owned()),
        );
        assert_eq!(
            lines,
            [
                "Container a  Stopping",
                "Container a  Stopped",
                "Network n  Removed"
            ]
        );
    }

    #[test]
    fn caps_the_log_at_a_line_boundary() {
        let mut log = String::new();
        let line = "x".repeat(1023);
        for _ in 0..64 {
            append_line(&mut log, &line);
        }
        assert_eq!(log.len(), LOG_LIMIT);
        append_line(&mut log, "last");
        assert!(log.len() <= LOG_LIMIT);
        assert!(log.starts_with(&line));
        assert!(log.ends_with(&format!("{line}\nlast\n")));
    }

    #[test]
    fn begin_replaces_the_previous_run() {
        let logs = ComposeDownLogs::default();
        logs.begin("oasis26");
        logs.append("oasis26", "old");
        logs.begin("oasis26");
        logs.append("oasis26", "new");
        assert_eq!(logs.read("oasis26"), "new\n");
        assert_eq!(logs.read("other"), "");
    }
}
