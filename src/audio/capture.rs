use anyhow::Result;

#[derive(Debug, Clone, Copy)]
pub struct LoopbackCaptureConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub frame_size: usize,
}

impl Default for LoopbackCaptureConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            frame_size: 1_024,
        }
    }
}

pub trait AudioCapture {
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct LoopbackCapture {
    pub config: LoopbackCaptureConfig,
    started: bool,
}

impl LoopbackCapture {
    pub fn new(config: LoopbackCaptureConfig) -> Self {
        Self {
            config,
            started: false,
        }
    }

    pub fn is_started(&self) -> bool {
        self.started
    }
}

impl AudioCapture for LoopbackCapture {
    fn start(&mut self) -> Result<()> {
        self.started = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.started = false;
        Ok(())
    }
}
