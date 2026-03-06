# dglab-socket-protocol

Reusable Rust helpers for the DG-LAB 3.0 websocket control protocol.

Crate name on crates.io:

```toml
dglab-socket-protocol = "0.1"
```

Import path in Rust code:

```rust
use dglab_socket_protocol::...;
```

## What this crate provides

- QR payload and websocket session URL helpers for DG-LAB app pairing
- protocol constants, packet types, and error codes
- builders for `strength-*`, `clear-*`, and `pulse-*` messages
- parser for app-reported `strength-a+b+softA+softB`
- reusable DG-LAB-compatible websocket server
- shared `DglabChannel` channel type

This crate only covers the socket protocol layer. Audio capture, DSP, GUI, and application logic stay outside.

## Modules

- `pairing`
  Build websocket URLs, generate QR payloads, rotate session IDs, and detect a LAN host.
- `protocol`
  Build outbound control messages, parse inbound strength reports, and work with websocket packets.
- `server`
  Run a reusable DG-LAB-compatible websocket server so downstream users do not need to implement the bind / heartbeat / forwarding flow themselves.

## Pairing helpers

```rust
use dglab_socket_protocol::pairing;

let ws_url = pairing::ws_url_for_host_and_new_session("192.168.50.229");
let qr_payload = pairing::build_qr_payload(&ws_url);

assert!(qr_payload.starts_with(
    "https://www.dungeon-lab.com/app-download.php#DGLAB-SOCKET#ws://"
));
```

The QR payload format matches the DG-LAB app requirement:

```text
https://www.dungeon-lab.com/app-download.php#DGLAB-SOCKET#ws://host:port/session-id
```

## Protocol helpers

```rust
use dglab_socket_protocol::{
    DglabChannel,
    protocol::{
        StrengthControlMode, build_clear_message, build_pulse_message,
        build_strength_message, parse_strength_report,
    },
};

let strength = build_strength_message(
    DglabChannel::A,
    StrengthControlMode::SetValue,
    35,
);
assert_eq!(strength, "strength-1+2+35");

let clear = build_clear_message(DglabChannel::B);
assert_eq!(clear, "clear-2");

let pulse = build_pulse_message(
    DglabChannel::A,
    "0A0A0A0A00000000 0A0A0A0A64646464",
)
.unwrap();
assert_eq!(
    pulse,
    "pulse-A:[\"0A0A0A0A00000000\",\"0A0A0A0A64646464\"]"
);

let report = parse_strength_report("strength-11+7+100+35").unwrap();
assert_eq!(report.a_strength, 11);
assert_eq!(report.b_soft_limit, 35);
```

## Reusable websocket server

The `server` module implements the common DG-LAB websocket flow:

- assign app ID on connect
- validate session ID from the websocket path
- handle `bind`
- answer `heartbeat`
- forward app `msg` packets to your program
- forward your outbound messages back to the bound app

Minimal example:

```rust
use dglab_socket_protocol::{
    pairing,
    server::{DglabWsServer, DglabWsServerConfig, DglabWsServerEvent},
};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let controller_id = "7e04d0a7-b6c0-4fa1-b255-5055c47b3374".to_owned();
    let ws_url = format!("ws://0.0.0.0:28846/{controller_id}");
    let qr_payload = pairing::build_qr_payload(&ws_url);

    println!("Scan this in the DG-LAB app:");
    println!("{qr_payload}");

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let server = DglabWsServer::new(
        DglabWsServerConfig::new("0.0.0.0:28846", controller_id),
        event_tx,
    );

    let control = server.control();
    tokio::spawn(server.run());

    while let Some(event) = event_rx.recv().await {
        match event {
            DglabWsServerEvent::Bound { .. } => {
                control.send_app_message("strength-1+2+20")?;
            }
            DglabWsServerEvent::AppMessage { message, .. } => {
                println!("app -> program: {message}");
            }
            _ => {}
        }
    }

    Ok(())
}
```

## Pulse frame rules

DG-LAB V3 pulse frames are strict:

- one pulse item must be exactly 16 hex chars
- first 8 chars are 4 frequency bytes
- last 8 chars are 4 waveform-strength bytes
- each frequency byte must stay in the valid V3 range
- invalid pulse data may cause the app or device to drop the whole frame

This crate validates pulse item length and hex format, but waveform semantics are still your responsibility.

## Current scope

This crate is extracted from `dglab-audio-link` and is intended to be a reusable protocol layer. It currently targets the DG-LAB 3.0 websocket protocol and does not attempt to wrap unrelated device-side BLE logic.
