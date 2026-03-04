use crate::dglab::pairing;
use crate::domain::{
    BAND_COUNT,
    types::{BandRouting, DglabChannel, StrengthRange},
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub websocket_url: String,
    pub band_routing: [BandRouting; BAND_COUNT],
    pub band_values: [f32; BAND_COUNT],
    pub strength_range: StrengthRange,
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
            strength_range: StrengthRange::new(10, 160),
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
