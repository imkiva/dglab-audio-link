# AGENTS Notes for dglab-audio-link

## Scope
- Platform target is Windows first.
- Audio source is speaker loopback capture, not microphone input.
- WebSocket pairing uses DG-LAB QR payload format.

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

## UI layout conventions
- Keep Band routing controls outside Settings panel.
- Keep waveform controls in a dedicated panel (separate from strength range).
- Settings panel should host operational settings (speaker source, protocol debug).
- Protocol debug panel should be collapsible and default collapsed.

## Persistence conventions
- Persist user settings in `settings.json` next to the executable.
- Keep schema migration explicit; current schema includes waveform mode and smoothing semantics.

## Verification checklist
- Run `cargo test` before commit.
- Manual test after pairing:
  1. Verify app receives strength reports for A/B and soft limits.
  2. Verify waveform appears in app for both auto pulse modes.
  3. Verify no visible gaps in active continuous waveform mode.

