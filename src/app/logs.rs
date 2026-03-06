use std::{
    collections::VecDeque,
    io::{self, Write},
    sync::{Arc, Mutex},
};

use tracing_subscriber::{EnvFilter, Registry, reload};

const MAX_LOG_LINES: usize = 4_000;

#[derive(Debug, Clone)]
pub struct GuiLogEntry {
    pub id: u64,
    pub level: GuiLogLevel,
    pub timestamp: String,
    pub target: String,
    pub message: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiLogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl GuiLogLevel {
    pub const fn all() -> [Self; 5] {
        [
            Self::Error,
            Self::Warn,
            Self::Info,
            Self::Debug,
            Self::Trace,
        ]
    }

    pub const fn directive(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    pub fn from_filter_text(filter_text: &str) -> Self {
        let normalized = filter_text.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "error" => Self::Error,
            "warn" => Self::Warn,
            "debug" => Self::Debug,
            "trace" => Self::Trace,
            _ => Self::Info,
        }
    }

    pub fn from_log_line(line: &str) -> Self {
        let uppercase = line.to_ascii_uppercase();
        if uppercase.contains(" ERROR ") {
            Self::Error
        } else if uppercase.contains(" WARN ") {
            Self::Warn
        } else if uppercase.contains(" DEBUG ") {
            Self::Debug
        } else if uppercase.contains(" TRACE ") {
            Self::Trace
        } else {
            Self::Info
        }
    }

    pub fn from_level_token(token: &str) -> Option<Self> {
        match token.trim().to_ascii_uppercase().as_str() {
            "ERROR" => Some(Self::Error),
            "WARN" => Some(Self::Warn),
            "INFO" => Some(Self::Info),
            "DEBUG" => Some(Self::Debug),
            "TRACE" => Some(Self::Trace),
            _ => None,
        }
    }
}

pub type GuiLogReloadHandle = reload::Handle<EnvFilter, Registry>;

#[derive(Clone, Default)]
pub struct GuiLogBuffer {
    inner: Arc<Mutex<GuiLogBufferInner>>,
}

#[derive(Default)]
struct GuiLogBufferInner {
    entries: VecDeque<GuiLogEntry>,
    next_id: u64,
    partial_line: String,
}

impl GuiLogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<GuiLogEntry> {
        self.inner
            .lock()
            .map(|inner| inner.entries.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.entries.clear();
            inner.partial_line.clear();
        }
    }

    fn push_text(&self, text: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            for chunk in text.split_inclusive('\n') {
                let chunk = chunk.trim_end_matches('\r');
                if let Some(line) = chunk.strip_suffix('\n') {
                    inner.partial_line.push_str(line.trim_end_matches('\r'));
                    let completed = std::mem::take(&mut inner.partial_line);
                    let entry = parse_log_entry(inner.next_id, completed);
                    inner.next_id = inner.next_id.saturating_add(1);
                    inner.entries.push_back(entry);
                } else {
                    inner.partial_line.push_str(chunk);
                }
            }

            while inner.entries.len() > MAX_LOG_LINES {
                inner.entries.pop_front();
            }
        }
    }
}

fn parse_log_entry(id: u64, text: String) -> GuiLogEntry {
    let trimmed = text.trim();
    let mut parts = trimmed.splitn(3, ' ');
    let first = parts.next().unwrap_or_default();
    let second = parts.next().unwrap_or_default();
    let third = parts.next().unwrap_or_default();

    let (timestamp, level, remainder) = if let Some(level) = GuiLogLevel::from_level_token(second) {
        (first.to_owned(), level, third)
    } else {
        (String::new(), GuiLogLevel::from_log_line(trimmed), trimmed)
    };

    let (target, message) = if let Some((target, message)) = remainder.split_once(": ") {
        (target.trim().to_owned(), message.to_owned())
    } else {
        (String::new(), remainder.to_owned())
    };

    GuiLogEntry {
        id,
        level,
        timestamp,
        target,
        message,
        text,
    }
}

pub struct GuiLogWriter {
    log_buffer: GuiLogBuffer,
    stderr: io::Stderr,
}

impl GuiLogWriter {
    pub fn new(log_buffer: GuiLogBuffer) -> Self {
        Self {
            log_buffer,
            stderr: io::stderr(),
        }
    }
}

impl Write for GuiLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        self.log_buffer.push_text(&text);
        self.stderr.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stderr.flush()
    }
}
