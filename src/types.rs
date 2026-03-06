pub use dglab_socket_protocol::DglabChannel;
use serde::{Deserialize, Serialize};

pub const BAND_COUNT: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AutoPulseMode {
    #[default]
    ByStrength,
    AlwaysMax,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BandDriveMode {
    #[default]
    Energy,
    Onset,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BandRouting {
    pub enabled: bool,
    pub threshold: f32,
    pub channel: DglabChannel,
    pub attack_ms: u16,
    pub hold_ms: u16,
    pub release_ms: u16,
}

impl BandRouting {
    pub const fn new(enabled: bool, threshold: f32, channel: DglabChannel) -> Self {
        Self {
            enabled,
            threshold,
            channel,
            attack_ms: 60,
            hold_ms: 140,
            release_ms: 260,
        }
    }
}

impl Default for BandRouting {
    fn default() -> Self {
        Self::new(true, 0.5, DglabChannel::A)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrengthRange {
    pub min: u16,
    pub max: u16,
}

impl StrengthRange {
    pub const fn new(min: u16, max: u16) -> Self {
        Self { min, max }
    }

    pub fn normalized(self) -> Self {
        let min = self.min.min(200);
        let max = self.max.min(200);
        if min <= max {
            Self { min, max }
        } else {
            Self { min: max, max: min }
        }
    }
}

impl Default for StrengthRange {
    fn default() -> Self {
        Self { min: 0, max: 200 }
    }
}
