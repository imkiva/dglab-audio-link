# AGENTS Notes for dglab-audio-link

## Scope
- Platform target is Windows first.
- Audio source is speaker loopback capture, not microphone input.
- WebSocket pairing uses DG-LAB QR payload format.
- Windows release UX should be GUI-only; do not reintroduce a console window on startup.

## Current structure
- Reusable DG-LAB websocket logic lives in `crates/dglab-socket-protocol`.
- Main app should import protocol helpers directly from `dglab_socket_protocol`; there is no local app-side `dglab` module anymore.
- Shared app-level types live in `src/types.rs`; do not recreate `src/domain/mod.rs` / `src/domain/types.rs`.
- Band-to-strength mapping lives in `src/audio/mapper.rs`; do not recreate `src/signal/*` or `src/pipeline/mapper.rs` unless there is a strong reason.
- Windows speaker enumeration lives in `src/audio/windows_endpoints.rs`.
- Windows loopback capture lives in `src/audio/windows_loopback.rs`.
- `src/audio/capture.rs` is now a platform-aware frontend that delegates to native WASAPI on Windows.

## Protocol-critical rules
- DGLab V3 pulse item is exactly 16 HEX chars: 8 chars frequency + 8 chars waveform strength.
- Frequency byte range must be 10..240 for each of the 4 slots.
- Waveform strength byte range must be 0..100 for each of the 4 slots.
- If one slot in a channel frame is invalid, the device can drop the whole channel frame for that 100ms window.
- Continuous baseline uses `0A0A0A0A` as frequency bytes.
- For active output, avoid sending full-zero waveform bytes unintentionally (`00000000`) because it creates audible/tactile gaps.

## App behavior constraints
- `Always max waveform` controls waveform amplitude only.
- Channel strength remains controlled by mapped band strength and strength range settings.
- Respect app soft limit per channel (A/B independently), do not collapse to `min(A, B)`.
- App strength updates come from `strength-a+b+softA+softB` messages; this is the source of slider caps.
- Engine settings must be synced before starting or restarting the pipeline worker; otherwise startup can trigger a duplicate audio-capture switch.
- Factory presets should not overwrite the current A/B strength ranges; user scene slots may persist and restore them.
- Audio analysis frame size is an operational capture setting, not a creative scene setting.

## UI layout conventions
- Keep Band routing controls outside Settings panel.
- Keep waveform controls in a dedicated panel (separate from strength range).
- Settings panel should host operational settings (speaker source, capture frame size, protocol debug).
- Protocol debug panel should be collapsible and default collapsed.
- Right-side log panel is independent from the main scroll area; left and right scrolling must not interfere.
- Expanding the log panel should grow the window width instead of squeezing the main content area.
- Log entries are structured list items, not a plain text blob.
- Log item display may be formatted, but copy/export must preserve raw plain-text log lines.
- Runtime log level is adjustable from the GUI and is intentionally not persisted.

## Persistence conventions
- Persist user settings in `settings.json` next to the executable.
- Keep schema migration explicit; current schema includes scene slots and analysis frame size.
- Do not persist GUI log viewer state such as log level, selection, or auto-scroll toggle.

## Verification checklist
- Run `cargo test` before commit.
- Manual test after pairing:
  1. Verify app receives strength reports for A/B and soft limits.
  2. Verify waveform appears in app for both auto pulse modes.
  3. Verify no visible gaps in active continuous waveform mode.
  4. Verify startup only performs one audio capture start for the selected speaker.
  5. Verify the log panel can copy/export raw text and runtime log-level changes take effect immediately.
  6. Verify Windows speaker selection uses native endpoint enumeration and lists devices that `cpal` may miss.
  7. Verify Windows loopback capture starts in event-driven WASAPI mode.
  8. Verify changing analysis frame size restarts capture and persists across relaunch.
