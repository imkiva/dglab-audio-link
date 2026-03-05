use crate::dglab::protocol::StrengthReport;
use crate::dglab::{pairing, protocol::StrengthControlMode};
use crate::domain::{
    BAND_COUNT,
    types::{BandRouting, DglabChannel, StrengthRange},
};

#[derive(Debug, Clone)]
pub struct AppState {
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
    pub auto_limit_with_app_soft_limit: bool,
    pub last_app_message: Option<String>,
    pub last_server_info: Option<String>,
    pub audio_capture_running: bool,
    pub audio_input_device: Option<String>,
    pub available_output_devices: Vec<String>,
    pub selected_output_device: Option<String>,
    pub running: bool,
    pub last_error: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            websocket_url: pairing::default_ws_url(),
            band_routing: [
                BandRouting::new(true, 0.25, DglabChannel::A),
                BandRouting::new(true, 0.35, DglabChannel::A),
                BandRouting::new(true, 0.45, DglabChannel::B),
                BandRouting::new(true, 0.55, DglabChannel::B),
            ],
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
            auto_limit_with_app_soft_limit: true,
            last_app_message: None,
            last_server_info: None,
            audio_capture_running: false,
            audio_input_device: None,
            available_output_devices: Vec::new(),
            selected_output_device: None,
            running: false,
            last_error: None,
        }
    }
}

impl AppState {
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
