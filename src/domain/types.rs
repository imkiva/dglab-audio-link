use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DglabChannel {
    #[default]
    A,
    B,
}

impl DglabChannel {
    pub fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }

    pub const fn pulse_symbol(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }

    pub const fn strength_channel_id(self) -> u8 {
        match self {
            Self::A => 1,
            Self::B => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandRouting {
    pub enabled: bool,
    pub threshold: f32,
    pub channel: DglabChannel,
}

impl BandRouting {
    pub const fn new(enabled: bool, threshold: f32, channel: DglabChannel) -> Self {
        Self {
            enabled,
            threshold,
            channel,
        }
    }
}

impl Default for BandRouting {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.5,
            channel: DglabChannel::A,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
