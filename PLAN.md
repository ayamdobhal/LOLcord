# Minimal Voice Chat — PLAN.md

## Goal
A lightweight voice + text chat client for Windows that doesn't eat your frames.
Discord replacement for gaming with friends. 10 users max.

## Non-Goals
- File sharing, screen share, video, rich embeds
- Mobile clients (for now)
- Federation, accounts, profiles
- Anything that makes it bloated

## Architecture

```
┌─────────┐     UDP (voice)     ┌──────────┐     UDP (voice)     ┌─────────┐
│ Client A │ ──────────────────► │  Server  │ ◄────────────────── │ Client B │
│          │ ◄────────────────── │          │ ──────────────────► │          │
│          │     TCP (signaling  │          │     TCP (signaling  │          │
│          │      + text chat)   │          │      + text chat)   │          │
└─────────┘                     └──────────┘                     └─────────┘
```

### Server
- Single binary, self-hosted
- **TCP** for signaling (join/leave/auth) and text messages
- **UDP** for voice relay — server mixes nothing, just forwards packets to all peers in a room
- Rooms (like channels) — user creates/joins by name + optional password
- Simple auth: username + room password (no accounts, no database)
- Max 10 concurrent users per room

### Client (Windows)
- Native GUI — **egui** (immediate mode, GPU-accelerated, tiny footprint)
- Opus encoding/decoding via `opus` crate (or `audiopus`)
- Audio capture/playback via `cpal` (cross-platform, WASAPI on Windows)
- Push-to-talk (configurable hotkey) + optional voice activity detection
- Minimal text chat panel alongside voice status
- System tray support — runs silently in background

## Tech Stack

| Component | Choice | Why |
|-----------|--------|-----|
| Language | Rust | Fast, low overhead, no GC |
| GUI | egui + eframe | Minimal, GPU-rendered, ~5MB binary |
| Audio I/O | cpal | WASAPI backend on Windows, low latency |
| Voice codec | Opus (audiopus) | Industry standard, great at low bitrate |
| Networking | tokio + std UDP | Async TCP signaling, raw UDP for voice |
| Serialization | bincode or MessagePack | Compact binary for voice packets, serde for signaling |

## Protocol

### Signaling (TCP, length-prefixed JSON messages)
```
→ { "type": "join", "username": "user1", "room": "gaming", "password": "optional" }
← { "type": "room_state", "users": ["user1", "user2"], "room": "gaming" }
← { "type": "user_joined", "username": "user2" }
← { "type": "user_left", "username": "user3" }
→ { "type": "chat", "text": "gg" }
← { "type": "chat", "from": "user1", "text": "gg", "ts": 1707... }
```

### Voice (UDP)
```
┌──────────┬───────────┬────────┬──────────────┐
│ room_id  │ user_id   │ seq_no │ opus_payload  │
│ (2 bytes)│ (2 bytes) │ (2 B)  │ (variable)   │
└──────────┴───────────┴────────┴──────────────┘
```
- Server receives UDP packet → forwards to all other users in same room
- No mixing, no transcoding — pure relay (SFU-style)
- Client handles jitter buffer + packet ordering via seq_no
- ~50 bytes overhead per packet, Opus frame every 20ms

## Project Structure

```
voicechat/
├── server/          # server binary (Cargo workspace member)
│   └── src/
│       ├── main.rs
│       ├── room.rs       # room management
│       ├── voice.rs      # UDP voice relay
│       └── signaling.rs  # TCP signaling + chat
├── client/          # client binary (Cargo workspace member)
│   └── src/
│       ├── main.rs
│       ├── ui.rs         # egui interface
│       ├── audio.rs      # capture + playback (cpal + opus)
│       ├── voice.rs      # UDP voice send/recv
│       └── signaling.rs  # TCP connection + chat
├── shared/          # shared types + protocol definitions
│   └── src/
│       └── lib.rs
├── Cargo.toml       # workspace root
└── PLAN.md
```

## MVP Milestones

### Phase 1 — Text Chat (foundation)
- [ ] Workspace setup, shared protocol types
- [ ] Server: TCP listener, room join/leave, broadcast text messages
- [ ] Client: egui window with connect screen + chat panel
- [ ] Basic flow: connect → join room → send/receive text

### Phase 2 — Voice
- [ ] Client: audio capture with cpal, Opus encoding
- [ ] Client: UDP send voice packets to server
- [ ] Server: UDP relay to all peers in room
- [ ] Client: receive + decode + playback
- [ ] Push-to-talk (configurable hotkey)
- [ ] Simple jitter buffer

### Phase 3 — Polish
- [ ] Voice activity detection (optional, alongside PTT)
- [ ] Per-user volume control
- [ ] System tray + minimize to tray
- [ ] Audio device selection
- [ ] Room passwords

## Global Keybinds

Must work even when the app is not focused (while in-game).

| Action | Default Key | Notes |
|--------|-------------|-------|
| Push-to-talk | `V` | Hold to transmit |
| Toggle mute | `Ctrl+Shift+M` | Mic on/off |
| Toggle deafen | `Ctrl+Shift+D` | Speakers on/off |

- Implemented via Windows raw input / `RegisterHotKey` (or `winapi`/`windows` crate)
- Keys fully configurable via settings
- Visual indicator in status bar when PTT active / muted / deafened

### Phase 4 — Nice to Have (later)
- [ ] Noise suppression (RNNoise via `nnnoiseless` crate)
- [ ] Encryption (voice packets — could use a shared room key + XChaCha20)
- [ ] Linux/macOS client builds
- [ ] Auto-reconnect on disconnect

## Performance Target
- **CPU:** <1% idle, <3% during active voice (vs Discord's 5-15%)
- **RAM:** <30MB (vs Discord's 300-500MB)
- **Binary:** <15MB
- **Latency:** <50ms voice end-to-end on LAN

## Open Questions
- [ ] Where to host the server? (your VPS, home server, etc.)
- [ ] Name for this thing?
- [ ] Do we want encryption from day 1 or add it later?
