pub use dglab_socket_protocol::DglabChannel;
use serde::{Deserialize, Serialize};

pub const BAND_COUNT: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandProfile {
    pub short_name_en: &'static str,
    pub short_name_zh: &'static str,
    pub detail_en: &'static str,
    pub detail_zh: &'static str,
    pub recommended_threshold: f32,
    pub low_hz: f32,
    pub focus_hz: f32,
    pub high_hz: f32,
}

impl BandProfile {
    pub const fn range_hz(self) -> (f32, f32) {
        (self.low_hz, self.high_hz)
    }
}

pub const BAND_PROFILES: [BandProfile; BAND_COUNT] = [
    BandProfile {
        short_name_en: "Kick / Sub",
        short_name_zh: "底鼓 / 下潜",
        detail_en: "Kick drum thump, sub drops, floor tom punch.",
        detail_zh: "底鼓冲击、下潜、落地嗵鼓的低频拳感。",
        recommended_threshold: 0.38,
        low_hz: 30.0,
        focus_hz: 55.0,
        high_hz: 110.0,
    },
    BandProfile {
        short_name_en: "Bass / Groove",
        short_name_zh: "贝斯 / 律动",
        detail_en: "Bass line movement, warm low-end body, lower synth groove.",
        detail_zh: "贝斯线条、低频温暖厚度、低音合成器律动。",
        recommended_threshold: 0.46,
        low_hz: 80.0,
        focus_hz: 150.0,
        high_hz: 280.0,
    },
    BandProfile {
        short_name_en: "Vocal / Lead",
        short_name_zh: "人声 / 主旋律",
        detail_en: "Vocal body and clarity, snare crack, guitar or piano lead.",
        detail_zh: "人声主体与清晰度、军鼓脆点、吉他或钢琴主旋律。",
        recommended_threshold: 0.32,
        low_hz: 250.0,
        focus_hz: 900.0,
        high_hz: 2_600.0,
    },
    BandProfile {
        short_name_en: "Hats / Air",
        short_name_zh: "镲片 / 空气感",
        detail_en: "Hi-hat, cymbal shimmer, breath noise, ambience sparkle.",
        detail_zh: "踩镲、镲片亮度、呼吸声和环境空气感细节。",
        recommended_threshold: 0.54,
        low_hz: 2_200.0,
        focus_hz: 6_000.0,
        high_hz: 12_000.0,
    },
];

pub const fn band_profile(index: usize) -> BandProfile {
    BAND_PROFILES[index]
}

pub const fn default_band_routing() -> [BandRouting; BAND_COUNT] {
    [
        BandRouting::new(
            true,
            BAND_PROFILES[0].recommended_threshold,
            DglabChannel::A,
        ),
        BandRouting::new(
            true,
            BAND_PROFILES[1].recommended_threshold,
            DglabChannel::A,
        ),
        BandRouting::new(
            true,
            BAND_PROFILES[2].recommended_threshold,
            DglabChannel::B,
        ),
        BandRouting::new(
            true,
            BAND_PROFILES[3].recommended_threshold,
            DglabChannel::B,
        ),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AutoPulseMode {
    #[default]
    ByStrength,
    AlwaysMax,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WaveformPatternMode {
    Fixed,
    #[default]
    AutoMorph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WaveformPattern {
    #[default]
    Smooth,
    Punch,
    Tide,
    Ripple,
    Shimmer,
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
