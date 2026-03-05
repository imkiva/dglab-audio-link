use std::{env, fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    app::{i18n::UiLanguage, state::AppState},
    domain::{
        BAND_COUNT,
        types::{AutoPulseMode, BandRouting, DglabChannel, StrengthRange},
    },
};

const SETTINGS_FILE_NAME: &str = "settings.json";
const SETTINGS_SCHEMA_VERSION: u8 = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedSettings {
    pub version: u8,
    pub language: UiLanguage,
    pub band_routing: [BandRouting; BAND_COUNT],
    pub strength_range_a: StrengthRange,
    pub strength_range_b: StrengthRange,
    pub auto_pulse_mode: AutoPulseMode,
    pub smooth_strength_enabled: bool,
    pub smooth_strength_factor: f32,
    pub selected_output_device: Option<String>,
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            version: 1,
            language: UiLanguage::En,
            band_routing: [
                BandRouting::new(true, 0.25, DglabChannel::A),
                BandRouting::new(true, 0.35, DglabChannel::A),
                BandRouting::new(true, 0.45, DglabChannel::B),
                BandRouting::new(true, 0.55, DglabChannel::B),
            ],
            strength_range_a: StrengthRange::new(10, 160),
            strength_range_b: StrengthRange::new(10, 160),
            auto_pulse_mode: AutoPulseMode::ByStrength,
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
        self.version = SETTINGS_SCHEMA_VERSION;

        self.strength_range_a = self.strength_range_a.normalized();
        self.strength_range_b = self.strength_range_b.normalized();
        self.smooth_strength_factor = self.smooth_strength_factor.clamp(0.0, 1.0);

        for route in &mut self.band_routing {
            route.threshold = route.threshold.clamp(0.0, 1.0);
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
