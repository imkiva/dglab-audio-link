# dglab-socket-protocol

Helpers for the DG-LAB 3.0 websocket control protocol.

This crate provides:

- QR payload and websocket session URL helpers
- protocol packet types and error codes
- strength / clear / pulse message builders
- strength report parsing
- a reusable DG-LAB-compatible websocket server
- `DglabChannel` channel type shared by protocol helpers

It is extracted from the `dglab-audio-link` application so the protocol layer can be reused independently.
