use serde::{Deserialize, Serialize};

/// Client → Server messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Join {
        username: String,
        room: String,
        #[serde(default)]
        password: Option<String>,
    },
    Leave,
    Chat {
        text: String,
    },
}

/// Server → Client messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    RoomState {
        room: String,
        users: Vec<String>,
        /// Assigned numeric user ID for voice packets
        user_id: u16,
        /// Assigned numeric room ID for voice packets
        room_id: u16,
        /// UDP port for voice traffic
        voice_port: u16,
    },
    UserJoined {
        username: String,
    },
    UserLeft {
        username: String,
    },
    Chat {
        from: String,
        text: String,
        ts: u64,
    },
    Error {
        message: String,
    },
}

/// Length-prefixed protocol helpers.
/// Wire format: [4-byte big-endian length][JSON payload]
pub mod protocol {
    use anyhow::{Context, Result, bail};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Write a length-prefixed JSON message.
    pub async fn write_message<W, T>(writer: &mut W, msg: &T) -> Result<()>
    where
        W: AsyncWriteExt + Unpin,
        T: serde::Serialize,
    {
        let payload = serde_json::to_vec(msg)?;
        let len = payload.len() as u32;
        writer.write_all(&len.to_be_bytes()).await?;
        writer.write_all(&payload).await?;
        writer.flush().await?;
        Ok(())
    }

    /// Read a length-prefixed JSON message.
    pub async fn read_message<R, T>(reader: &mut R) -> Result<T>
    where
        R: AsyncReadExt + Unpin,
        T: serde::de::DeserializeOwned,
    {
        let mut len_buf = [0u8; 4];
        reader
            .read_exact(&mut len_buf)
            .await
            .context("connection closed")?;
        let len = u32::from_be_bytes(len_buf) as usize;

        if len > 1024 * 64 {
            bail!("message too large: {len} bytes");
        }

        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).await?;
        let msg = serde_json::from_slice(&buf)?;
        Ok(msg)
    }
}

pub const MAX_ROOM_USERS: usize = 10;
pub const DEFAULT_PORT: u16 = 7480;
pub const DEFAULT_VOICE_PORT: u16 = 7481;

/// Voice UDP packet header (6 bytes) + opus payload.
///
/// Wire format:
/// ```text
/// [room_id: u16][user_id: u16][seq_no: u16][opus_payload...]
/// ```
pub mod voice {
    pub const HEADER_SIZE: usize = 6;
    pub const MAX_PACKET_SIZE: usize = 1400; // safe for MTU

    /// Sample rate for Opus (48kHz mono).
    pub const SAMPLE_RATE: u32 = 48000;
    /// Frame duration in ms.
    pub const FRAME_DURATION_MS: usize = 20;
    /// Samples per frame (48000 * 20ms = 960).
    pub const FRAME_SIZE: usize = (SAMPLE_RATE as usize * FRAME_DURATION_MS) / 1000;
    /// Channels (mono).
    pub const CHANNELS: u16 = 1;

    pub fn encode_header(room_id: u16, user_id: u16, seq: u16) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..2].copy_from_slice(&room_id.to_be_bytes());
        buf[2..4].copy_from_slice(&user_id.to_be_bytes());
        buf[4..6].copy_from_slice(&seq.to_be_bytes());
        buf
    }

    pub fn decode_header(buf: &[u8]) -> Option<(u16, u16, u16)> {
        if buf.len() < HEADER_SIZE {
            return None;
        }
        let room_id = u16::from_be_bytes([buf[0], buf[1]]);
        let user_id = u16::from_be_bytes([buf[2], buf[3]]);
        let seq = u16::from_be_bytes([buf[4], buf[5]]);
        Some((room_id, user_id, seq))
    }
}

// voice module above provides packet encoding/decoding

