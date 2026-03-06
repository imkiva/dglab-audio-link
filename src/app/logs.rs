use std::{
    collections::VecDeque,
    io::{self, Write},
    sync::{Arc, Mutex},
};

use tracing_subscriber::{EnvFilter, Registry, reload};

const MAX_LOG_LINES: usize = 4_000;

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
}

pub type GuiLogReloadHandle = reload::Handle<EnvFilter, Registry>;

#[derive(Clone, Default)]
pub struct GuiLogBuffer {
    inner: Arc<Mutex<GuiLogBufferInner>>,
}

#[derive(Default)]
struct GuiLogBufferInner {
    lines: VecDeque<String>,
    partial_line: String,
}

impl GuiLogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.inner
            .lock()
            .map(|inner| inner.lines.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.lines.clear();
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
                    inner.lines.push_back(completed);
                } else {
                    inner.partial_line.push_str(chunk);
                }
            }

            while inner.lines.len() > MAX_LOG_LINES {
                inner.lines.pop_front();
            }
        }
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
