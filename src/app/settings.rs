use std::{env, fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    app::{i18n::UiLanguage, state::AppState},
    types::{
        AutoPulseMode, BAND_COUNT, BandDriveMode, BandRouting, StrengthRange, WaveformPattern,
        WaveformPatternMode, default_band_routing,
    },
};

const SETTINGS_FILE_NAME: &str = "settings.json";
const SETTINGS_SCHEMA_VERSION: u8 = 7;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedSettings {
    pub version: u8,
    pub language: UiLanguage,
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
    pub selected_output_device: Option<String>,
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            version: 1,
            language: UiLanguage::En,
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
            selected_output_device: None,
        }
    }
}

impl PersistedSettings {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            version: SETTINGS_SCHEMA_VERSION,
            language: state.language,
            band_routing: state.band_routing,
            strength_range_a: state.strength_range_a,
            strength_range_b: state.strength_range_b,
            auto_pulse_mode: state.auto_pulse_mode,
            band_drive_mode: state.band_drive_mode,
            waveform_pattern_mode: state.waveform_pattern_mode,
            waveform_pattern: state.waveform_pattern,
            waveform_contrast: state.waveform_contrast,
            smooth_strength_enabled: state.smooth_strength_enabled,
            smooth_strength_factor: state.smooth_strength_factor,
            selected_output_device: state.selected_output_device.clone(),
        }
        .sanitized()
    }

    pub fn apply_to_state(&self, state: &mut AppState) {
        let normalized = self.clone().sanitized();
        state.language = normalized.language;
        state.band_routing = normalized.band_routing;
        state.strength_range_a = normalized.strength_range_a;
        state.strength_range_b = normalized.strength_range_b;
        state.auto_pulse_mode = normalized.auto_pulse_mode;
        state.band_drive_mode = normalized.band_drive_mode;
        state.waveform_pattern_mode = normalized.waveform_pattern_mode;
        state.waveform_pattern = normalized.waveform_pattern;
        state.waveform_contrast = normalized.waveform_contrast;
        state.smooth_strength_enabled = normalized.smooth_strength_enabled;
        state.smooth_strength_factor = normalized.smooth_strength_factor;
        state.selected_output_device = normalized.selected_output_device;
    }

    fn sanitized(mut self) -> Self {
        if self.version < 2 {
            // v1 used inverse semantics for smooth strength factor.
            self.smooth_strength_factor = 1.0 - self.smooth_strength_factor.clamp(0.0, 1.0);
        }
        if self.version < 3 {
            self.auto_pulse_mode = AutoPulseMode::ByStrength;
        }
        if self.version < 4 {
            self.waveform_contrast = 1.8;
        }
        if self.version < 5 {
            for route in &mut self.band_routing {
                route.attack_ms = route.attack_ms.clamp(0, 2_000);
                route.hold_ms = route.hold_ms.clamp(0, 2_000);
                route.release_ms = route.release_ms.clamp(0, 2_000);
            }
        }
        if self.version < 6 {
            self.band_drive_mode = BandDriveMode::Energy;
        }
        if self.version < 7 {
            self.waveform_pattern_mode = WaveformPatternMode::AutoMorph;
            self.waveform_pattern = WaveformPattern::Smooth;
        }
        self.version = SETTINGS_SCHEMA_VERSION;

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

        self.selected_output_device = self.selected_output_device.and_then(|name| {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        });
        self
    }
}

pub fn load_settings() -> Result<Option<PersistedSettings>, String> {
    let path = settings_file_path();
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read settings file `{}`: {err}", path.display()))?;
    let settings: PersistedSettings = serde_json::from_str(&content)
        .map_err(|err| format!("failed to parse settings file `{}`: {err}", path.display()))?;
    Ok(Some(settings.sanitized()))
}

pub fn save_settings(settings: &PersistedSettings) -> Result<(), String> {
    let path = settings_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create settings directory `{}`: {err}",
                parent.display()
            )
        })?;
    }

    let content = serde_json::to_string_pretty(&settings.clone().sanitized())
        .map_err(|err| format!("failed to serialize settings: {err}"))?;
    fs::write(&path, content)
        .map_err(|err| format!("failed to write settings file `{}`: {err}", path.display()))
}

fn settings_file_path() -> PathBuf {
    if let Ok(exe_path) = env::current_exe() {
        if let Some(parent) = exe_path.parent() {
            return parent.join(SETTINGS_FILE_NAME);
        }
    }

    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(SETTINGS_FILE_NAME)
}
