# dglab-audio-link

Rust binary project (nightly toolchain) for:

- Capturing Windows speaker output.
- Converting audio into 4 bands (`0.0..=1.0`).
- Applying per-band trigger threshold.
- Mapping triggered bands to DGLab 3.0 channel `A` or `B`.
- Generating DGLab pairing QR payload for mobile app scan.
- Serving websocket endpoint for app connection.

## Toolchain

- Rust: `nightly-2025-07-12`
- OS target (current phase): Windows only
- GUI: `eframe/egui` (lightweight, native desktop)

## Project layout

```text
src/
  app/
    gui.rs          # GUI app and widgets
    state.rs        # editable runtime state in GUI
  audio/
    capture.rs      # Windows loopback capture skeleton
    analyzer.rs     # 4-band analyzer skeleton
  dglab/
    pairing.rs      # QR payload format and session URL helpers
    protocol.rs     # websocket payload skeleton
    server.rs       # websocket server skeleton
  domain/
    types.rs        # shared domain types and constraints
  signal/
    mapper.rs       # band -> intensity mapping logic
  pipeline/
    engine.rs       # orchestration skeleton
  main.rs           # app entry point
```

## Current status

- Project structure and compile-time skeleton are ready.
- Program WS URL now defaults to auto-detected LAN IPv4 (fallback `127.0.0.1`).
- GUI provides `Use Local LAN IP` to refresh host address while preserving port/session path.
- Program startup automatically launches the local websocket server (`0.0.0.0:<port>`).
- GUI contains pairing QR generation for `https://www.dungeon-lab.com/app-download.php#DGLAB-SOCKET#<ws-url>`.
- GUI contains fields for websocket URL, 4 bands, thresholds, A/B mapping, and strength range (`0..200`).
- Websocket server now handles DGLab protocol flow: connect -> bind(targetId) -> bind(DGLAB) -> bind(200), heartbeat, msg validation and error codes.
- Signal mapping logic has basic unit tests.
- Audio capture remains a scaffold for the next implementation step.
