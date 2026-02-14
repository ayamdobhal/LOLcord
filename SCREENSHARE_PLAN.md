# Screen Share Implementation Plan

## Specs
- 720p (1280x720) @ 30fps
- H.264 encoding via openh264
- Self-preview for sharer
- Fullscreen/popout viewer option

## Protocol Changes (shared/)

### New UDP Packet Types
Current voice packet: `[room_id:u16][user_id:u16][seq:u16][opus_data...]`

New screen packet format:
```
[0xFF][0xFE]           -- magic bytes (distinguish from voice)
[room_id:u16]          -- room
[user_id:u16]          -- who's sharing
[frame_id:u32]         -- frame sequence number
[fragment_idx:u16]     -- fragment index within frame
[fragment_count:u16]   -- total fragments in this frame
[is_keyframe:u8]       -- 1 if keyframe, 0 if not
[payload...]           -- H.264 NAL unit fragment
```

Fragment size: ~1200 bytes (safe for MTU, avoid IP fragmentation)
A 720p30 keyframe ~50-100KB = ~40-80 fragments
P-frames ~5-20KB = ~4-16 fragments

### New TCP Messages
```
ClientMessage::StartScreenShare
ClientMessage::StopScreenShare
ServerMessage::ScreenShareStarted { username }
ServerMessage::ScreenShareStopped { username }
```

## Server Changes (server/)
- Track which user (if any) is screen sharing per room (1 share at a time)
- Relay screen UDP packets to all room peers (same as voice)
- Handle StartScreenShare/StopScreenShare TCP messages
- Broadcast ScreenShareStarted/Stopped to room

## Client Changes (client/)

### New Crates
- `scrap` — screen capture (DXGI on Windows)
- `openh264` — H.264 encode/decode

### Capture Thread (sharer)
1. `scrap::Capturer` captures frames as BGRA
2. Convert BGRA → YUV420 (openh264 input format)
3. Encode with openh264 encoder (720p, 30fps, ~2-4 Mbps bitrate)
4. Fragment encoded NAL units into ~1200 byte UDP packets
5. Send via existing UDP socket with screen packet header
6. Also push decoded frame to self-preview

### Receive + Decode (viewer)
1. Separate screen packets from voice in UDP recv loop
2. Reassemble fragments into complete frames (buffer by frame_id)
3. Discard incomplete frames after timeout (100ms)
4. Decode H.264 frame via openh264 decoder
5. Convert YUV420 → RGBA for display
6. Push to UI as iced::widget::image

### UI
- "Share Screen" button in sidebar (below mute/deafen)
- When sharing: button changes to "Stop Sharing", self-preview shows in chat area
- When viewing someone's share: stream replaces chat area
- "Fullscreen" button on the stream view → opens separate window or maximizes
- "Pop Out" → separate native window (iced supports multi-window? if not, skip for now)
- Tab/toggle to switch between chat and screen share view

## Phases
1. Protocol + server relay + capture + encode + send
2. Receive + decode + display + self-preview
3. Fullscreen/popout, resolution picker, FPS control

## File Structure
```
client/src/
  screen_capture.rs  — capture thread, BGRA→YUV, encode, fragment
  screen_decode.rs   — reassemble, decode, YUV→RGBA
  ui.rs              — new UI elements for screen share
```
