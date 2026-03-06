use dglab_socket_protocol::{
    pairing,
    protocol::{StrengthControlMode, StrengthReport},
};

use crate::app::{
    i18n::UiLanguage,
    scenes::{SavedScene, SceneConfig, empty_scene_slots},
};
use crate::audio::capture::{DEFAULT_ANALYSIS_FRAME_SIZE, normalize_analysis_frame_size};
use crate::types::{
    AutoPulseMode, BAND_COUNT, BandDriveMode, BandRouting, DglabChannel, StrengthRange,
    WaveformPattern, WaveformPatternMode, default_band_routing,
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub language: UiLanguage,
    pub websocket_url: String,
    pub band_routing: [BandRouting; BAND_COUNT],
    pub band_values: [f32; BAND_COUNT],
    pub strength_range_a: StrengthRange,
    pub strength_range_b: StrengthRange,
    pub debug_strength_channel: DglabChannel,
    pub debug_strength_mode: StrengthControlMode,
    pub debug_strength_value: u16,
    pub debug_clear_channel: DglabChannel,
    pub debug_pulse_channel: DglabChannel,
    pub debug_pulse_values: String,
    pub last_protocol_action: Option<String>,
    pub app_connected: bool,
    pub app_bound: bool,
    pub app_id: Option<String>,
    pub app_strength_report: Option<StrengthReport>,
    pub output_strengths: [u16; 2],
    pub auto_limit_with_app_soft_limit: bool,
    pub auto_pulse_mode: AutoPulseMode,
    pub band_drive_mode: BandDriveMode,
    pub waveform_pattern_mode: WaveformPatternMode,
    pub waveform_pattern: WaveformPattern,
    pub waveform_contrast: f32,
    pub smooth_strength_enabled: bool,
    pub smooth_strength_factor: f32,
    pub saved_scenes: Vec<Option<SavedScene>>,
    pub last_app_message: Option<String>,
    pub last_server_info: Option<String>,
    pub audio_capture_running: bool,
    pub audio_input_device: Option<String>,
    pub analysis_frame_size: usize,
    pub available_output_devices: Vec<String>,
    pub selected_output_device: Option<String>,
    pub running: bool,
    pub last_error: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            language: UiLanguage::default(),
            websocket_url: pairing::default_ws_url(),
            band_routing: default_band_routing(),
            band_values: [0.0; BAND_COUNT],
            strength_range_a: StrengthRange::new(10, 160),
            strength_range_b: StrengthRange::new(10, 160),
            debug_strength_channel: DglabChannel::A,
            debug_strength_mode: StrengthControlMode::SetValue,
            debug_strength_value: 50,
            debug_clear_channel: DglabChannel::A,
            debug_pulse_channel: DglabChannel::A,
            debug_pulse_values:
                "0A0A0A0A0A0A0A0A 0A0A0A0A0A0A0A0A 0A0A0A0A0A0A0A0A 0A0A0A0A0A0A0A0A".to_owned(),
            last_protocol_action: None,
            app_connected: false,
            app_bound: false,
            app_id: None,
            app_strength_report: None,
            output_strengths: [0; 2],
            auto_limit_with_app_soft_limit: true,
            auto_pulse_mode: AutoPulseMode::ByStrength,
            band_drive_mode: BandDriveMode::Energy,
            waveform_pattern_mode: WaveformPatternMode::AutoMorph,
            waveform_pattern: WaveformPattern::Smooth,
            waveform_contrast: 1.8,
            smooth_strength_enabled: true,
            smooth_strength_factor: 0.70,
            saved_scenes: empty_scene_slots(),
            last_app_message: None,
            last_server_info: None,
            audio_capture_running: false,
            audio_input_device: None,
            analysis_frame_size: DEFAULT_ANALYSIS_FRAME_SIZE,
            available_output_devices: Vec::new(),
            selected_output_device: None,
            running: false,
            last_error: None,
        }
    }
}

impl AppState {
    pub fn capture_scene_config(&self) -> SceneConfig {
        SceneConfig {
            band_routing: self.band_routing,
            strength_range_a: self.strength_range_a,
            strength_range_b: self.strength_range_b,
            auto_pulse_mode: self.auto_pulse_mode,
            band_drive_mode: self.band_drive_mode,
            waveform_pattern_mode: self.waveform_pattern_mode,
            waveform_pattern: self.waveform_pattern,
            waveform_contrast: self.waveform_contrast,
            smooth_strength_enabled: self.smooth_strength_enabled,
            smooth_strength_factor: self.smooth_strength_factor,
        }
        .sanitized()
    }

    pub fn apply_scene_config(&mut self, config: &SceneConfig) {
        let config = config.sanitized();
        self.band_routing = config.band_routing;
        self.strength_range_a = config.strength_range_a;
        self.strength_range_b = config.strength_range_b;
        self.auto_pulse_mode = config.auto_pulse_mode;
        self.band_drive_mode = config.band_drive_mode;
        self.waveform_pattern_mode = config.waveform_pattern_mode;
        self.waveform_pattern = config.waveform_pattern;
        self.waveform_contrast = config.waveform_contrast;
        self.smooth_strength_enabled = config.smooth_strength_enabled;
        self.smooth_strength_factor = config.smooth_strength_factor;
    }

    pub fn apply_factory_scene_config(&mut self, config: &SceneConfig) {
        let preserved_a = self.strength_range_a;
        let preserved_b = self.strength_range_b;
        self.apply_scene_config(config);
        self.strength_range_a = preserved_a;
        self.strength_range_b = preserved_b;
    }

    pub fn clear_error(&mut self) {
        self.last_error = None;
    }

    pub fn set_error(&mut self, message: impl Into<String>) {
        self.last_error = Some(message.into());
    }

    pub fn set_protocol_action(&mut self, message: impl Into<String>) {
        self.last_protocol_action = Some(message.into());
    }

    pub fn app_soft_limit_for_channel(&self, channel: DglabChannel) -> Option<u16> {
        let report = self.app_strength_report?;
        match channel {
            DglabChannel::A => Some(report.a_soft_limit),
            DglabChannel::B => Some(report.b_soft_limit),
        }
    }

    pub fn app_current_strength_for_channel(&self, channel: DglabChannel) -> Option<u16> {
        let report = self.app_strength_report?;
        match channel {
            DglabChannel::A => Some(report.a_strength),
            DglabChannel::B => Some(report.b_strength),
        }
    }

    pub fn effective_strength_slider_max_for_channel(&self, channel: DglabChannel) -> u16 {
        if !self.auto_limit_with_app_soft_limit {
            return 200;
        }
        self.app_soft_limit_for_channel(channel).unwrap_or(200)
    }

    pub fn effective_debug_strength_slider_max(&self, channel: DglabChannel) -> u16 {
        if !self.auto_limit_with_app_soft_limit {
            return 200;
        }
        self.app_soft_limit_for_channel(channel).unwrap_or(200)
    }

    pub fn normalized_smooth_strength_factor(&self) -> f32 {
        self.smooth_strength_factor.clamp(0.0, 1.0)
    }

    pub fn normalized_waveform_contrast(&self) -> f32 {
        self.waveform_contrast.clamp(1.0, 4.0)
    }

    pub fn normalized_analysis_frame_size(&self) -> usize {
        normalize_analysis_frame_size(self.analysis_frame_size)
    }

    pub fn rotate_session_id(&mut self) {
        self.websocket_url = pairing::rotate_session_id_in_ws_url(&self.websocket_url);
    }

    pub fn refresh_lan_ws_url(&mut self) -> bool {
        if let Some(url) = pairing::auto_detect_lan_ws_url(&self.websocket_url) {
            self.websocket_url = url;
            true
        } else {
            self.websocket_url =
                pairing::replace_host_in_ws_url(&self.websocket_url, pairing::FALLBACK_HOST);
            false
        }
    }
}
