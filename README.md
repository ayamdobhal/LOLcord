# LOLcord 🐱🎧

Lightweight voice + text chat for gaming. Built in Rust.

## Features

- **Voice chat** — Opus codec, SFU-style relay (no mixing), jitter buffer
- **Text chat** — real-time messaging alongside voice
- **Push to Talk / Open Mic** — configurable PTT keybinds (keyboard + mouse buttons), VAD with sensitivity control
- **Per-user volume** — 0-200% per person
- **Noise suppression** — RNNoise-based, toggleable
- **Noise gate** — adjustable threshold
- **E2E encryption** — XChaCha20-Poly1305, password-derived keys (Argon2id)
- **System tray** — minimize to tray, mute/deafen from tray menu
- **Auto-reconnect** — exponential backoff on disconnect
- **Connection quality** — live ping (ms) and packet loss (%) display
- **Settings persistence** — saved to `%APPDATA%/.lolcord/`

## Architecture

```
workspace/
├── shared/   # Protocol definitions (TCP messages, wire format)
├── server/   # SFU relay server (TCP signaling + UDP voice)
└── client/   # egui desktop client (Windows)
```

- **Server**: Pure relay — receives UDP voice packets, forwards to room peers. No mixing, no transcoding.
- **Client**: egui GUI, cpal audio, Opus encode/decode, tokio networking.

## Building

### Server (macOS/Linux)

```bash
cargo build -p server --release
```

### Client (Windows, cross-compiled from macOS)

```bash
rustup target add x86_64-pc-windows-gnu
cargo build -p client --target x86_64-pc-windows-gnu --release
```

## Running

### Server

```bash
# Default: TCP 7480, UDP 7481
./target/release/server

# With config file
./target/release/server --config server.toml
```

### Client

Run `LOLcord.exe` on Windows. Enter server address, username, and room name.

## Config

Server supports TOML config and environment variable overrides. See `server.toml.example` for options.

## License

MIT
