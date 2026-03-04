use serde::{Deserialize, Serialize};

pub const MAX_JSON_CHARS: usize = 1_950;
pub const MESSAGE_TARGET_ID: &str = "targetId";
pub const MESSAGE_DGLAB: &str = "DGLAB";

pub const CODE_OK: &str = "200";
pub const CODE_QR_CLIENT_ID_INVALID: &str = "210";
pub const CODE_BIND_TARGET_NOT_FOUND: &str = "401";
pub const CODE_NOT_BOUND: &str = "402";
pub const CODE_INVALID_JSON: &str = "403";
pub const CODE_RECEIVER_OFFLINE: &str = "404";
pub const CODE_MESSAGE_TOO_LONG: &str = "405";
pub const CODE_INTERNAL_ERROR: &str = "500";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Heartbeat,
    Bind,
    Msg,
    Break,
    Error,
}

impl PacketType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Heartbeat => "heartbeat",
            Self::Bind => "bind",
            Self::Msg => "msg",
            Self::Break => "break",
            Self::Error => "error",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "heartbeat" => Some(Self::Heartbeat),
            "bind" => Some(Self::Bind),
            "msg" => Some(Self::Msg),
            "break" => Some(Self::Break),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SocketPacket {
    #[serde(rename = "type")]
    pub packet_type: String,
    #[serde(rename = "clientId")]
    pub client_id: String,
    #[serde(rename = "targetId")]
    pub target_id: String,
    pub message: String,
}

impl SocketPacket {
    pub fn new(
        packet_type: impl Into<String>,
        client_id: impl Into<String>,
        target_id: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            packet_type: packet_type.into(),
            client_id: client_id.into(),
            target_id: target_id.into(),
            message: message.into(),
        }
    }

    pub fn from_type(
        packet_type: PacketType,
        client_id: impl Into<String>,
        target_id: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(packet_type.as_str(), client_id, target_id, message)
    }

    pub fn bind_assign_current_id(current_id: impl Into<String>) -> Self {
        Self::from_type(PacketType::Bind, current_id, "", MESSAGE_TARGET_ID)
    }

    pub fn bind_result(
        client_id: impl Into<String>,
        target_id: impl Into<String>,
        code: impl Into<String>,
    ) -> Self {
        Self::from_type(PacketType::Bind, client_id, target_id, code)
    }

    pub fn heartbeat(
        client_id: impl Into<String>,
        target_id: impl Into<String>,
        code: impl Into<String>,
    ) -> Self {
        Self::from_type(PacketType::Heartbeat, client_id, target_id, code)
    }

    pub fn msg(
        client_id: impl Into<String>,
        target_id: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::from_type(PacketType::Msg, client_id, target_id, message)
    }

    pub fn break_packet(
        client_id: impl Into<String>,
        target_id: impl Into<String>,
        code: impl Into<String>,
    ) -> Self {
        Self::from_type(PacketType::Break, client_id, target_id, code)
    }

    pub fn error(
        client_id: impl Into<String>,
        target_id: impl Into<String>,
        code: impl Into<String>,
    ) -> Self {
        Self::from_type(PacketType::Error, client_id, target_id, code)
    }

    pub fn kind(&self) -> Option<PacketType> {
        PacketType::parse(self.packet_type.trim())
    }

    pub fn has_required_non_empty_values(&self) -> bool {
        !self.packet_type.trim().is_empty()
            && !self.client_id.trim().is_empty()
            && !self.target_id.trim().is_empty()
            && !self.message.trim().is_empty()
    }
}
