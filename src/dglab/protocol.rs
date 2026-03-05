use serde::{Deserialize, Serialize};

use crate::domain::types::DglabChannel;

pub const MAX_JSON_CHARS: usize = 1_950;
pub const MESSAGE_TARGET_ID: &str = "targetId";
pub const MESSAGE_DGLAB: &str = "DGLAB";
pub const MAX_PULSE_ITEMS: usize = 100;
pub const PULSE_HEX_CHARS: usize = 16;

pub const CODE_OK: &str = "200";
pub const CODE_QR_CLIENT_ID_INVALID: &str = "210";
pub const CODE_BIND_TARGET_NOT_FOUND: &str = "401";
pub const CODE_NOT_BOUND: &str = "402";
pub const CODE_INVALID_JSON: &str = "403";
pub const CODE_RECEIVER_OFFLINE: &str = "404";
pub const CODE_MESSAGE_TOO_LONG: &str = "405";
pub const CODE_INTERNAL_ERROR: &str = "500";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StrengthControlMode {
    Decrease,
    Increase,
    #[default]
    SetValue,
}

impl StrengthControlMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Decrease => "Decrease",
            Self::Increase => "Increase",
            Self::SetValue => "Set",
        }
    }

    pub const fn opcode(self) -> u8 {
        match self {
            Self::Decrease => 0,
            Self::Increase => 1,
            Self::SetValue => 2,
        }
    }
}

pub fn build_strength_message(
    channel: DglabChannel,
    mode: StrengthControlMode,
    value: u16,
) -> String {
    format!(
        "strength-{}+{}+{}",
        channel.strength_channel_id(),
        mode.opcode(),
        value.min(200)
    )
}

pub fn build_clear_message(channel: DglabChannel) -> String {
    format!("clear-{}", channel.strength_channel_id())
}

pub fn build_pulse_message(channel: DglabChannel, raw_hex_items: &str) -> Result<String, String> {
    let mut items = Vec::new();
    for token in raw_hex_items.split(|c: char| c.is_whitespace() || [',', ';', '|'].contains(&c)) {
        let item = token.trim();
        if item.is_empty() {
            continue;
        }
        let normalized = item.to_ascii_uppercase();
        items.push(normalized);
    }

    build_pulse_message_from_items(channel, &items)
}

pub fn build_pulse_message_from_items(
    channel: DglabChannel,
    raw_items: &[String],
) -> Result<String, String> {
    let mut items = Vec::with_capacity(raw_items.len());
    for item in raw_items {
        let normalized = item.trim().to_ascii_uppercase();
        if normalized.len() != PULSE_HEX_CHARS
            || !normalized.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Err(format!(
                "invalid pulse item `{item}`. each item must be 16 HEX chars, e.g. 0A0A0A0A00000000"
            ));
        }
        items.push(normalized);
    }

    if items.is_empty() {
        return Err("no pulse item provided".to_owned());
    }
    if items.len() > MAX_PULSE_ITEMS {
        return Err(format!(
            "too many pulse items: {} (max {MAX_PULSE_ITEMS})",
            items.len()
        ));
    }

    let json_array = serde_json::to_string(&items).map_err(|err| err.to_string())?;
    Ok(format!("pulse-{}:{json_array}", channel.pulse_symbol()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrengthReport {
    pub a_strength: u16,
    pub b_strength: u16,
    pub a_soft_limit: u16,
    pub b_soft_limit: u16,
}

pub fn parse_strength_report(message: &str) -> Option<StrengthReport> {
    let trimmed = message.trim();
    let prefix = trimmed.get(..9)?;
    if !prefix.eq_ignore_ascii_case("strength-") {
        return None;
    }

    let payload = trimmed.get(9..)?;
    let mut parts = payload.split('+');
    let a_strength = parts.next()?.parse::<u16>().ok()?;
    let b_strength = parts.next()?.parse::<u16>().ok()?;
    let a_soft_limit = parts.next()?.parse::<u16>().ok()?;
    let b_soft_limit = parts.next()?.parse::<u16>().ok()?;
    if parts.next().is_some() {
        return None;
    }

    if [a_strength, b_strength, a_soft_limit, b_soft_limit]
        .iter()
        .any(|value| *value > 200)
    {
        return None;
    }

    Some(StrengthReport {
        a_strength,
        b_strength,
        a_soft_limit,
        b_soft_limit,
    })
}

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

#[cfg(test)]
mod tests {
    use super::{
        StrengthControlMode, build_clear_message, build_pulse_message, build_strength_message,
        parse_strength_report,
    };
    use crate::domain::types::DglabChannel;

    #[test]
    fn builds_strength_message() {
        let msg = build_strength_message(DglabChannel::A, StrengthControlMode::SetValue, 35);
        assert_eq!(msg, "strength-1+2+35");
    }

    #[test]
    fn builds_clear_message() {
        let msg = build_clear_message(DglabChannel::B);
        assert_eq!(msg, "clear-2");
    }

    #[test]
    fn builds_pulse_message_from_text() {
        let msg = build_pulse_message(DglabChannel::A, "0A0A0A0A00000000, a1b2c3d4e5f60718")
            .expect("must pass");
        assert_eq!(msg, "pulse-A:[\"0A0A0A0A00000000\",\"A1B2C3D4E5F60718\"]");
    }

    #[test]
    fn rejects_invalid_pulse_item() {
        let err = build_pulse_message(DglabChannel::A, "123").expect_err("must fail");
        assert!(err.contains("invalid pulse item"));
    }

    #[test]
    fn parses_strength_report() {
        let report = parse_strength_report("strength-11+7+100+35").expect("must parse");
        assert_eq!(report.a_strength, 11);
        assert_eq!(report.b_strength, 7);
        assert_eq!(report.a_soft_limit, 100);
        assert_eq!(report.b_soft_limit, 35);
    }

    #[test]
    fn parses_strength_report_case_insensitive_prefix() {
        let report = parse_strength_report("StReNgTh-11+7+100+35").expect("must parse");
        assert_eq!(report.a_strength, 11);
        assert_eq!(report.b_strength, 7);
        assert_eq!(report.a_soft_limit, 100);
        assert_eq!(report.b_soft_limit, 35);
    }
}
