use serde::{Deserialize, Serialize};

use crate::types::{
    AutoPulseMode, BAND_COUNT, BandDriveMode, BandRouting, DglabChannel, StrengthRange,
    WaveformPattern, WaveformPatternMode, default_band_routing,
};

pub const USER_SCENE_SLOT_COUNT: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FactorySceneId {
    #[default]
    BalancedMotion,
    EdmDrop,
    TechnoRail,
    HouseLift,
    HipHopBounce,
    PopVocalBloom,
    AirSparkle,
    RockDriver,
    DnBRush,
    AmbientTide,
    LoFiDrift,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SceneConfig {
    pub band_routing: [BandRouting; BAND_COUNT],
    pub strength_range_a: StrengthRange,
    pub strength_range_b: StrengthRange,
    pub auto_pulse_mode: AutoPulseMode,
    pub band_drive_mode: BandDriveMode,
    pub waveform_pattern_mode: WaveformPatternMode,
    pub waveform_pattern: WaveformPattern,
    pub waveform_contrast: f32,
    pub smooth_strength_enabled: bool,
    pub smooth_strength_factor: f32,
}

impl Default for SceneConfig {
    fn default() -> Self {
        Self {
            band_routing: default_band_routing(),
            strength_range_a: StrengthRange::new(10, 160),
            strength_range_b: StrengthRange::new(10, 160),
            auto_pulse_mode: AutoPulseMode::ByStrength,
            band_drive_mode: BandDriveMode::Energy,
            waveform_pattern_mode: WaveformPatternMode::AutoMorph,
            waveform_pattern: WaveformPattern::Smooth,
            waveform_contrast: 1.8,
            smooth_strength_enabled: true,
            smooth_strength_factor: 0.70,
        }
    }
}

impl SceneConfig {
    pub fn sanitized(mut self) -> Self {
        self.strength_range_a = self.strength_range_a.normalized();
        self.strength_range_b = self.strength_range_b.normalized();
        self.waveform_contrast = self.waveform_contrast.clamp(1.0, 4.0);
        self.smooth_strength_factor = self.smooth_strength_factor.clamp(0.0, 1.0);

        for route in &mut self.band_routing {
            route.threshold = route.threshold.clamp(0.0, 1.0);
            route.attack_ms = route.attack_ms.clamp(0, 2_000);
            route.hold_ms = route.hold_ms.clamp(0, 2_000);
            route.release_ms = route.release_ms.clamp(0, 2_000);
        }

        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedScene {
    pub name: String,
    pub config: SceneConfig,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FactoryScenePreset {
    pub id: FactorySceneId,
    pub name_en: &'static str,
    pub name_zh: &'static str,
    pub summary_en: &'static str,
    pub summary_zh: &'static str,
    pub config: SceneConfig,
}

const fn route(
    threshold: f32,
    channel: DglabChannel,
    attack_ms: u16,
    hold_ms: u16,
    release_ms: u16,
) -> BandRouting {
    BandRouting {
        enabled: true,
        threshold,
        channel,
        attack_ms,
        hold_ms,
        release_ms,
    }
}

const fn scene(
    band_routing: [BandRouting; BAND_COUNT],
    strength_range_a: StrengthRange,
    strength_range_b: StrengthRange,
    auto_pulse_mode: AutoPulseMode,
    band_drive_mode: BandDriveMode,
    waveform_pattern_mode: WaveformPatternMode,
    waveform_pattern: WaveformPattern,
    waveform_contrast: f32,
    smooth_strength_enabled: bool,
    smooth_strength_factor: f32,
) -> SceneConfig {
    SceneConfig {
        band_routing,
        strength_range_a,
        strength_range_b,
        auto_pulse_mode,
        band_drive_mode,
        waveform_pattern_mode,
        waveform_pattern,
        waveform_contrast,
        smooth_strength_enabled,
        smooth_strength_factor,
    }
}

pub const FACTORY_SCENE_PRESETS: [FactoryScenePreset; 11] = [
    FactoryScenePreset {
        id: FactorySceneId::BalancedMotion,
        name_en: "Balanced Motion",
        name_zh: "均衡律动",
        summary_en: "General music preset with balanced movement and auto-morph waveform.",
        summary_zh: "适合大多数音乐的均衡预设，带自动形变波形。",
        config: scene(
            default_band_routing(),
            StrengthRange::new(10, 160),
            StrengthRange::new(10, 160),
            AutoPulseMode::ByStrength,
            BandDriveMode::Energy,
            WaveformPatternMode::AutoMorph,
            WaveformPattern::Smooth,
            1.8,
            true,
            0.70,
        ),
    },
    FactoryScenePreset {
        id: FactorySceneId::EdmDrop,
        name_en: "EDM Drop",
        name_zh: "电音 Drop",
        summary_en: "Big-room style: hard kick impact, bright tops, fast transient reaction.",
        summary_zh: "偏大房电音：底鼓冲击强、顶部亮、瞬态反应快。",
        config: scene(
            [
                route(0.28, DglabChannel::A, 15, 90, 150),
                route(0.40, DglabChannel::A, 20, 100, 170),
                route(0.44, DglabChannel::B, 24, 110, 180),
                route(0.60, DglabChannel::B, 12, 70, 120),
            ],
            StrengthRange::new(28, 185),
            StrengthRange::new(16, 150),
            AutoPulseMode::ByStrength,
            BandDriveMode::Onset,
            WaveformPatternMode::Fixed,
            WaveformPattern::Punch,
            2.8,
            true,
            0.22,
        ),
    },
    FactoryScenePreset {
        id: FactorySceneId::TechnoRail,
        name_en: "Techno Rail",
        name_zh: "Techno 脉冲",
        summary_en: "Steady low-end engine with a linear rolling pulse for hypnotic techno grooves.",
        summary_zh: "持续低频引擎感，滚动脉冲更适合催眠式 techno 律动。",
        config: scene(
            [
                route(0.34, DglabChannel::A, 35, 130, 220),
                route(0.38, DglabChannel::A, 40, 140, 240),
                route(0.46, DglabChannel::B, 32, 110, 180),
                route(0.58, DglabChannel::B, 22, 80, 130),
            ],
            StrengthRange::new(22, 170),
            StrengthRange::new(18, 140),
            AutoPulseMode::ByStrength,
            BandDriveMode::Energy,
            WaveformPatternMode::Fixed,
            WaveformPattern::Ripple,
            2.0,
            true,
            0.58,
        ),
    },
    FactoryScenePreset {
        id: FactorySceneId::HouseLift,
        name_en: "House Lift",
        name_zh: "House 抬升",
        summary_en: "Four-on-the-floor kick with clap and hat lift, tuned for house and disco-pop.",
        summary_zh: "四拍底鼓配合 clap 和帽子上扬，更适合 house 和 disco-pop。",
        config: scene(
            [
                route(0.32, DglabChannel::A, 18, 100, 160),
                route(0.42, DglabChannel::A, 28, 110, 170),
                route(0.34, DglabChannel::B, 24, 130, 200),
                route(0.46, DglabChannel::B, 14, 85, 135),
            ],
            StrengthRange::new(20, 170),
            StrengthRange::new(14, 150),
            AutoPulseMode::ByStrength,
            BandDriveMode::Onset,
            WaveformPatternMode::Fixed,
            WaveformPattern::Tide,
            2.1,
            true,
            0.42,
        ),
    },
    FactoryScenePreset {
        id: FactorySceneId::HipHopBounce,
        name_en: "Hip-Hop Bounce",
        name_zh: "嘻哈弹跳",
        summary_en: "Heavy sub and bass hold longer, with less top-end chatter and a slower bounce.",
        summary_zh: "超低频和贝斯停留更久，高频更克制，整体更慢更弹。",
        config: scene(
            [
                route(0.30, DglabChannel::A, 60, 180, 320),
                route(0.40, DglabChannel::A, 70, 190, 330),
                route(0.42, DglabChannel::B, 55, 150, 260),
                route(0.72, DglabChannel::B, 30, 90, 140),
            ],
            StrengthRange::new(30, 175),
            StrengthRange::new(10, 125),
            AutoPulseMode::ByStrength,
            BandDriveMode::Energy,
            WaveformPatternMode::Fixed,
            WaveformPattern::Punch,
            1.9,
            true,
            0.68,
        ),
    },
    FactoryScenePreset {
        id: FactorySceneId::PopVocalBloom,
        name_en: "Pop Vocal Bloom",
        name_zh: "流行人声绽放",
        summary_en: "Pushes vocal and lead presence forward while keeping bass supportive instead of dominant.",
        summary_zh: "把人声和主旋律推到前面，让低频更多扮演陪衬而不是主导。",
        config: scene(
            [
                route(0.58, DglabChannel::A, 50, 120, 230),
                route(0.52, DglabChannel::A, 55, 140, 240),
                route(0.24, DglabChannel::B, 45, 180, 300),
                route(0.58, DglabChannel::B, 35, 100, 170),
            ],
            StrengthRange::new(12, 125),
            StrengthRange::new(22, 155),
            AutoPulseMode::ByStrength,
            BandDriveMode::Energy,
            WaveformPatternMode::Fixed,
            WaveformPattern::Tide,
            1.55,
            true,
            0.80,
        ),
    },
    FactoryScenePreset {
        id: FactorySceneId::AirSparkle,
        name_en: "Air Sparkle",
        name_zh: "空气闪烁",
        summary_en: "Highlights hats, shimmer, and top-end details with a brighter pulse feel.",
        summary_zh: "突出踩镲、亮度和高频细节，波形更轻快明亮。",
        config: scene(
            [
                route(0.60, DglabChannel::A, 40, 100, 180),
                route(0.54, DglabChannel::A, 35, 100, 180),
                route(0.40, DglabChannel::B, 30, 110, 200),
                route(0.44, DglabChannel::B, 18, 90, 150),
            ],
            StrengthRange::new(10, 110),
            StrengthRange::new(22, 145),
            AutoPulseMode::ByStrength,
            BandDriveMode::Onset,
            WaveformPatternMode::Fixed,
            WaveformPattern::Shimmer,
            2.1,
            true,
            0.45,
        ),
    },
    FactoryScenePreset {
        id: FactorySceneId::RockDriver,
        name_en: "Rock Driver",
        name_zh: "摇滚推进",
        summary_en: "Snare crack, guitar midrange, and kick all stay active without turning the mix into pure sub.",
        summary_zh: "保留军鼓脆点、吉他中频和底鼓推进，不会只剩一团低频。",
        config: scene(
            [
                route(0.34, DglabChannel::A, 20, 110, 170),
                route(0.44, DglabChannel::A, 30, 120, 190),
                route(0.28, DglabChannel::B, 24, 140, 220),
                route(0.56, DglabChannel::B, 18, 80, 130),
            ],
            StrengthRange::new(24, 170),
            StrengthRange::new(18, 165),
            AutoPulseMode::ByStrength,
            BandDriveMode::Onset,
            WaveformPatternMode::Fixed,
            WaveformPattern::Ripple,
            2.2,
            true,
            0.34,
        ),
    },
    FactoryScenePreset {
        id: FactorySceneId::DnBRush,
        name_en: "Drum & Bass Rush",
        name_zh: "鼓打贝斯疾冲",
        summary_en: "Very fast breakbeat scene with aggressive highs and transient-focused top-end energy.",
        summary_zh: "更快的 breakbeat 场景，高频更凶，瞬态导向更明显。",
        config: scene(
            [
                route(0.40, DglabChannel::A, 10, 70, 110),
                route(0.52, DglabChannel::A, 14, 80, 120),
                route(0.30, DglabChannel::B, 14, 85, 130),
                route(0.34, DglabChannel::B, 8, 65, 100),
            ],
            StrengthRange::new(20, 160),
            StrengthRange::new(26, 180),
            AutoPulseMode::ByStrength,
            BandDriveMode::Onset,
            WaveformPatternMode::Fixed,
            WaveformPattern::Shimmer,
            2.6,
            true,
            0.15,
        ),
    },
    FactoryScenePreset {
        id: FactorySceneId::AmbientTide,
        name_en: "Ambient Tide",
        name_zh: "氛围潮汐",
        summary_en: "Slow swells and broad movement for pads, drones, ambient pop, and cinematic wash.",
        summary_zh: "适合 pad、drone、氛围流行和电影化铺底，强调缓慢起伏。",
        config: scene(
            [
                route(0.62, DglabChannel::A, 140, 280, 520),
                route(0.54, DglabChannel::A, 120, 260, 500),
                route(0.38, DglabChannel::B, 110, 280, 520),
                route(0.46, DglabChannel::B, 100, 240, 450),
            ],
            StrengthRange::new(8, 95),
            StrengthRange::new(8, 105),
            AutoPulseMode::ByStrength,
            BandDriveMode::Energy,
            WaveformPatternMode::AutoMorph,
            WaveformPattern::Smooth,
            1.2,
            true,
            0.92,
        ),
    },
    FactoryScenePreset {
        id: FactorySceneId::LoFiDrift,
        name_en: "Lo-fi Drift",
        name_zh: "低保真漂移",
        summary_en: "Soft, lazy movement that suits lo-fi beats, mellow keys, and dusty background groove.",
        summary_zh: "更柔、更懒、更慢，适合 lo-fi beat、温和键盘和尘感背景律动。",
        config: scene(
            [
                route(0.48, DglabChannel::A, 100, 220, 420),
                route(0.44, DglabChannel::A, 110, 240, 450),
                route(0.36, DglabChannel::B, 95, 220, 380),
                route(0.68, DglabChannel::B, 90, 150, 260),
            ],
            StrengthRange::new(10, 110),
            StrengthRange::new(10, 100),
            AutoPulseMode::ByStrength,
            BandDriveMode::Energy,
            WaveformPatternMode::Fixed,
            WaveformPattern::Smooth,
            1.25,
            true,
            0.86,
        ),
    },
];

pub fn factory_scene_preset(id: FactorySceneId) -> &'static FactoryScenePreset {
    match id {
        FactorySceneId::BalancedMotion => &FACTORY_SCENE_PRESETS[0],
        FactorySceneId::EdmDrop => &FACTORY_SCENE_PRESETS[1],
        FactorySceneId::TechnoRail => &FACTORY_SCENE_PRESETS[2],
        FactorySceneId::HouseLift => &FACTORY_SCENE_PRESETS[3],
        FactorySceneId::HipHopBounce => &FACTORY_SCENE_PRESETS[4],
        FactorySceneId::PopVocalBloom => &FACTORY_SCENE_PRESETS[5],
        FactorySceneId::AirSparkle => &FACTORY_SCENE_PRESETS[6],
        FactorySceneId::RockDriver => &FACTORY_SCENE_PRESETS[7],
        FactorySceneId::DnBRush => &FACTORY_SCENE_PRESETS[8],
        FactorySceneId::AmbientTide => &FACTORY_SCENE_PRESETS[9],
        FactorySceneId::LoFiDrift => &FACTORY_SCENE_PRESETS[10],
    }
}

pub fn empty_scene_slots() -> Vec<Option<SavedScene>> {
    vec![None; USER_SCENE_SLOT_COUNT]
}

pub fn normalize_saved_scenes(mut scenes: Vec<Option<SavedScene>>) -> Vec<Option<SavedScene>> {
    scenes.truncate(USER_SCENE_SLOT_COUNT);
    while scenes.len() < USER_SCENE_SLOT_COUNT {
        scenes.push(None);
    }

    for (index, slot) in scenes.iter_mut().enumerate() {
        if let Some(scene) = slot.as_mut() {
            let trimmed = scene.name.trim();
            scene.name = if trimmed.is_empty() {
                format!("Scene {}", index + 1)
            } else {
                trimmed.to_owned()
            };
            scene.config = scene.config.sanitized();
        }
    }

    scenes
}

#[cfg(test)]
mod tests {
    use super::{
        FACTORY_SCENE_PRESETS, FactorySceneId, SavedScene, SceneConfig, empty_scene_slots,
        factory_scene_preset, normalize_saved_scenes,
    };

    #[test]
    fn factory_presets_cover_each_id() {
        assert_eq!(
            factory_scene_preset(FactorySceneId::BalancedMotion).id,
            FactorySceneId::BalancedMotion
        );
        assert_eq!(
            factory_scene_preset(FactorySceneId::EdmDrop).id,
            FactorySceneId::EdmDrop
        );
        assert_eq!(
            factory_scene_preset(FactorySceneId::TechnoRail).id,
            FactorySceneId::TechnoRail
        );
        assert_eq!(
            factory_scene_preset(FactorySceneId::HouseLift).id,
            FactorySceneId::HouseLift
        );
        assert_eq!(
            factory_scene_preset(FactorySceneId::HipHopBounce).id,
            FactorySceneId::HipHopBounce
        );
        assert_eq!(
            factory_scene_preset(FactorySceneId::PopVocalBloom).id,
            FactorySceneId::PopVocalBloom
        );
        assert_eq!(
            factory_scene_preset(FactorySceneId::AirSparkle).id,
            FactorySceneId::AirSparkle
        );
        assert_eq!(
            factory_scene_preset(FactorySceneId::RockDriver).id,
            FactorySceneId::RockDriver
        );
        assert_eq!(
            factory_scene_preset(FactorySceneId::DnBRush).id,
            FactorySceneId::DnBRush
        );
        assert_eq!(
            factory_scene_preset(FactorySceneId::AmbientTide).id,
            FactorySceneId::AmbientTide
        );
        assert_eq!(
            factory_scene_preset(FactorySceneId::LoFiDrift).id,
            FactorySceneId::LoFiDrift
        );
        assert_eq!(FACTORY_SCENE_PRESETS.len(), 11);
    }

    #[test]
    fn normalizes_scene_slots_and_names() {
        let normalized = normalize_saved_scenes(vec![Some(SavedScene {
            name: "   ".to_owned(),
            config: SceneConfig::default(),
        })]);
        assert_eq!(normalized.len(), 4);
        assert_eq!(
            normalized[0].as_ref().map(|scene| scene.name.as_str()),
            Some("Scene 1")
        );
    }

    #[test]
    fn creates_empty_scene_slots() {
        let slots = empty_scene_slots();
        assert_eq!(slots.len(), 4);
        assert!(slots.iter().all(Option::is_none));
    }
}
