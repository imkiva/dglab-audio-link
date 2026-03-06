# dglab-audio-link

**OpenAI Codex 24K 纯 Token 匠心打造，代码含人量 0%**

Rust desktop app (nightly toolchain) for:

- Capturing Windows speaker output.
- Converting playback into 4 music-oriented bands (`0.0..=1.0`).
- Mapping triggered bands to DGLab 3.0 channel `A` or `B`.
- Generating DGLab pairing QR payload for mobile app scan.
- Serving a local websocket endpoint for app pairing and control.

## Toolchain

- Rust: `nightly-2025-07-12`
- OS target (current phase): Windows only
- GUI: `eframe/egui` (lightweight, native desktop)

## Project layout

```text
crates/
  dglab-socket-protocol/
    src/
      pairing.rs    # QR payload format and session URL helpers
      protocol.rs   # websocket payload and parser helpers
      server.rs     # reusable DG-LAB websocket server
src/
  app/
    gui.rs          # GUI app and widgets
    scenes.rs       # factory presets and user scene slots
    settings.rs     # persisted GUI/app settings
    state.rs        # editable runtime state in GUI
  audio/
    capture.rs              # capture selection and shared frontend
    windows_endpoints.rs    # native Windows render endpoint enumeration
    windows_loopback.rs     # native WASAPI loopback capture backend
    analyzer.rs             # 4-band analyzer
    mapper.rs       # band -> intensity mapping logic
  pipeline/
    engine.rs       # orchestration and auto output pipeline
  types.rs          # shared app-level types and constants
  main.rs           # app entry point
```

## Current capabilities

- Program WS URL now defaults to auto-detected LAN IPv4 (fallback `127.0.0.1`).
- GUI provides `Use Local LAN IP` to refresh host address while preserving port/session path.
- Program startup automatically launches the local websocket server (`0.0.0.0:<port>`).
- GUI contains pairing QR generation for `https://www.dungeon-lab.com/app-download.php#DGLAB-SOCKET#<ws-url>`.
- GUI contains a manual protocol debug panel to send `strength-*`, `clear-*`, and `pulse-*`.
- GUI contains a structured log viewer with runtime log level control.
- GUI syncs app-reported `strength-a+b+softA+softB` and can auto-limit strength sliders by soft limits.
- Manual send avoids silent fail by checking app bind state and outbound JSON length (`<=1950`).
- Pulse debug input uses 16-hex items per frame (for example `0A0A0A0A00000000`).
- Automatic audio-reactive pipeline:
  - Native Windows render endpoint enumeration via `MMDeviceEnumerator`.
  - Native WASAPI loopback capture backend.
  - Event-driven WASAPI capture mode on Windows.
  - GUI speaker selection with hot-switching capture target.
  - Configurable analysis frame size: `256`, `512`, `1024`, `2048`.
  - FFT-based 4-band analyzer.
  - Per-band trigger, envelope, and A/B routing mapping.
  - Band drive mode: `Energy` or `Onset`.
  - Auto send `strength-*`, `pulse-*`, `clear-*` to DGLab app when bound.
  - Auto strength can clamp by app soft limit to reduce silent drop risk.
  - Auto waveform supports fixed patterns and auto morphing.
- Music-oriented band roles:
  - `Kick / Sub`
  - `Bass / Groove`
  - `Vocal / Lead`
  - `Hats / Air`
- Scene system:
  - Multiple factory presets for music styles
  - User scene save/load slots
  - Factory presets keep the current A/B strength ranges
- Websocket server handles DGLab protocol flow: connect -> bind(targetId) -> bind(DGLAB) -> bind(200), heartbeat, message validation, and error codes.

## Notes

- Audio source is speaker playback loopback, not microphone input.
- On Windows, the app uses native render endpoint enumeration because some devices are not listed reliably by `cpal` alone.
- Scene slots persist full creative settings. Factory presets intentionally do not overwrite the current A/B strength range.
