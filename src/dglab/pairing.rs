use std::net::{IpAddr, UdpSocket};

use uuid::Uuid;

pub const DGLAB_QR_PREFIX: &str = "https://www.dungeon-lab.com/app-download.php#DGLAB-SOCKET#";
pub const DEFAULT_BIND_PORT: u16 = 28_846;
pub const FALLBACK_HOST: &str = "127.0.0.1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlWsUrl {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub session_id: String,
}

pub fn default_ws_url() -> String {
    let host = detect_lan_host().unwrap_or_else(|| FALLBACK_HOST.to_owned());
    ws_url_for_host_and_new_session(&host)
}

pub fn detect_lan_host() -> Option<String> {
    for remote in ["1.1.1.1:80", "8.8.8.8:80", "223.5.5.5:80"] {
        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
            if socket.connect(remote).is_ok() {
                if let Ok(local_addr) = socket.local_addr() {
                    if let IpAddr::V4(ipv4) = local_addr.ip() {
                        if !ipv4.is_loopback() {
                            return Some(ipv4.to_string());
                        }
                    }
                }
            }
        }
    }

    None
}

pub fn replace_host_in_ws_url(ws_url: &str, new_host: &str) -> String {
    let parsed = parse_ws_url(ws_url);
    let scheme = parsed
        .as_ref()
        .map(|parts| parts.scheme.as_str())
        .unwrap_or("ws");
    let port = parsed
        .as_ref()
        .and_then(|parts| parts.port)
        .unwrap_or(DEFAULT_BIND_PORT);
    let path = parsed
        .as_ref()
        .map(|parts| parts.path.as_str())
        .unwrap_or_default();

    let session_path = if path.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        path.to_owned()
    };

    build_ws_url(scheme, new_host, port, &session_path)
}

pub fn auto_detect_lan_ws_url(ws_url: &str) -> Option<String> {
    detect_lan_host().map(|host| replace_host_in_ws_url(ws_url, &host))
}

pub fn parse_control_ws_url(ws_url: &str) -> Option<ControlWsUrl> {
    let parsed = parse_ws_url(ws_url)?;
    let port = parsed.port.unwrap_or(DEFAULT_BIND_PORT);
    let path = parsed.path.trim_matches('/');
    if path.is_empty() || path.contains('/') {
        return None;
    }

    Some(ControlWsUrl {
        scheme: parsed.scheme,
        host: parsed.host,
        port,
        session_id: path.to_owned(),
    })
}

pub fn ws_url_uses_loopback(ws_url: &str) -> bool {
    match parse_ws_url(ws_url) {
        Some(parts) => {
            parts.host.eq_ignore_ascii_case("localhost")
                || parts.host == "127.0.0.1"
                || parts.host == "::1"
        }
        None => true,
    }
}

fn build_ws_url(scheme: &str, host: &str, port: u16, path: &str) -> String {
    let clean_host = host.trim();
    let clean_path = path.trim().trim_start_matches('/');
    format!("{scheme}://{clean_host}:{port}/{clean_path}")
}

fn parse_ws_url(ws_url: &str) -> Option<WsUrlParts> {
    let trimmed = ws_url.trim();
    let (scheme, rest) = if let Some(value) = trimmed.strip_prefix("ws://") {
        ("ws", value)
    } else if let Some(value) = trimmed.strip_prefix("wss://") {
        ("wss", value)
    } else {
        return None;
    };

    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() {
        return None;
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port_str)) => {
            if host.is_empty() {
                return None;
            }
            let port = port_str.parse::<u16>().ok();
            (host.to_owned(), port)
        }
        None => (authority.to_owned(), None),
    };

    Some(WsUrlParts {
        scheme: scheme.to_owned(),
        host,
        port,
        path: path.to_owned(),
    })
}

#[derive(Debug, Clone)]
struct WsUrlParts {
    scheme: String,
    host: String,
    port: Option<u16>,
    path: String,
}

pub fn ws_url_for_host_and_new_session(host: &str) -> String {
    let session_id = Uuid::new_v4();
    build_ws_url("ws", host, DEFAULT_BIND_PORT, &session_id.to_string())
}

pub fn build_qr_payload(ws_url: &str) -> String {
    format!("{DGLAB_QR_PREFIX}{}", ws_url.trim())
}

pub fn rotate_session_id_in_ws_url(ws_url: &str) -> String {
    let session_id = Uuid::new_v4().to_string();
    let trimmed = ws_url.trim();
    if trimmed.is_empty() {
        return default_ws_url();
    }

    match trimmed.rsplit_once('/') {
        Some((base, _tail)) if base.starts_with("ws://") || base.starts_with("wss://") => {
            format!("{base}/{session_id}")
        }
        _ => default_ws_url(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_qr_payload, parse_control_ws_url, replace_host_in_ws_url,
        rotate_session_id_in_ws_url, ws_url_uses_loopback,
    };

    #[test]
    fn builds_expected_qr_payload_prefix() {
        let payload = build_qr_payload("ws://192.168.1.20:28846/abc");
        assert_eq!(
            payload,
            "https://www.dungeon-lab.com/app-download.php#DGLAB-SOCKET#ws://192.168.1.20:28846/abc"
        );
    }

    #[test]
    fn rotates_only_session_id_segment() {
        let next = rotate_session_id_in_ws_url("ws://192.168.1.20:28846/old-session");
        assert!(next.starts_with("ws://192.168.1.20:28846/"));
        assert_ne!(next, "ws://192.168.1.20:28846/old-session");
    }

    #[test]
    fn replaces_host_keeps_port_and_path() {
        let next = replace_host_in_ws_url("ws://192.168.1.20:28846/session-1", "10.0.0.66");
        assert_eq!(next, "ws://10.0.0.66:28846/session-1");
    }

    #[test]
    fn loopback_check_works_for_localhost() {
        assert!(ws_url_uses_loopback("ws://localhost:28846/a"));
        assert!(ws_url_uses_loopback("ws://127.0.0.1:28846/a"));
        assert!(!ws_url_uses_loopback("ws://192.168.1.20:28846/a"));
    }

    #[test]
    fn parses_control_ws_url_with_one_path_segment() {
        let parsed =
            parse_control_ws_url("ws://192.168.1.20:28846/7e04d0a7-b6c0-4fa1-b255-5055c47b3374")
                .expect("must parse");

        assert_eq!(parsed.scheme, "ws");
        assert_eq!(parsed.host, "192.168.1.20");
        assert_eq!(parsed.port, 28846);
        assert_eq!(parsed.session_id, "7e04d0a7-b6c0-4fa1-b255-5055c47b3374");
    }

    #[test]
    fn rejects_multi_segment_path_for_qr_protocol() {
        assert!(parse_control_ws_url("ws://192.168.1.20:28846/a/b").is_none());
    }
}
