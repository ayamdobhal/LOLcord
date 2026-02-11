use audiopus::{
    coder::Decoder,
    packet::Packet,
    Channels, MutSignals, SampleRate,
};
use shared::voice;
use std::collections::BTreeMap;

/// Per-user jitter buffer with its own Opus decoder for PLC.
struct UserBuffer {
    /// Buffered opus packets keyed by sequence number.
    packets: BTreeMap<u16, Vec<u8>>,
    decoder: Decoder,
    /// Next sequence number we expect to play.
    play_seq: u16,
    /// Whether we've accumulated enough initial delay.
    primed: bool,
    /// Number of packets received before we start playing.
    buffered_count: usize,
}

impl UserBuffer {
    fn new() -> Self {
        Self {
            packets: BTreeMap::new(),
            decoder: Decoder::new(SampleRate::Hz48000, Channels::Mono)
                .expect("failed to create opus decoder"),
            play_seq: 0,
            primed: false,
            buffered_count: 0,
        }
    }

    /// Insert a packet into the buffer.
    fn insert(&mut self, seq: u16, opus_data: Vec<u8>) {
        if !self.primed {
            if self.packets.is_empty() {
                self.play_seq = seq;
            }
            self.buffered_count += 1;
        }
        self.packets.insert(seq, opus_data);
        // Cap buffer size to prevent unbounded growth
        while self.packets.len() > 20 {
            self.packets.pop_first();
        }
    }

    /// Try to produce the next frame of decoded PCM.
    /// Returns None if not yet primed.
    fn next_frame(&mut self) -> Option<Vec<f32>> {
        // Need 3 frames (~60ms) buffered before we start
        if !self.primed {
            if self.buffered_count < 3 {
                return None;
            }
            self.primed = true;
        }

        let mut pcm_i16 = vec![0i16; voice::FRAME_SIZE];

        let decoded = if let Some(opus_data) = self.packets.remove(&self.play_seq) {
            // We have this packet — decode it
            let packet = Packet::try_from(opus_data.as_slice()).ok()?;
            let output = MutSignals::try_from(&mut pcm_i16[..]).ok()?;
            self.decoder.decode(Some(packet), output, false).ok()?
        } else {
            // Missing packet — use PLC
            let output = MutSignals::try_from(&mut pcm_i16[..]).ok()?;
            self.decoder.decode(None, output, false).unwrap_or(0)
        };

        self.play_seq = self.play_seq.wrapping_add(1);

        if decoded == 0 {
            return None;
        }

        let pcm_f32: Vec<f32> = pcm_i16[..decoded]
            .iter()
            .map(|&s| s as f32 / 32767.0)
            .collect();
        Some(pcm_f32)
    }

    /// Returns true if this buffer has been idle (no packets for a while).
    fn is_idle(&self) -> bool {
        self.packets.is_empty() && self.primed
    }
}

/// Jitter buffer managing multiple users.
pub struct JitterBuffer {
    users: std::collections::HashMap<u16, UserBuffer>,
    /// Per-user volume multiplier (0.0 to 2.0). Default 1.0.
    pub volumes: std::collections::HashMap<u16, f32>,
}

impl JitterBuffer {
    pub fn new() -> Self {
        Self {
            users: std::collections::HashMap::new(),
            volumes: std::collections::HashMap::new(),
        }
    }

    /// Insert an incoming voice packet.
    pub fn insert(&mut self, user_id: u16, seq: u16, opus_data: Vec<u8>) {
        let buf = self.users
            .entry(user_id)
            .or_insert_with(UserBuffer::new);
        buf.insert(seq, opus_data);
    }

    /// Produce mixed PCM for one 20ms frame from all users.
    /// Returns None if no audio to play.
    pub fn mix_frame(&mut self) -> Option<Vec<f32>> {
        let mut mixed: Option<Vec<f32>> = None;

        let user_ids: Vec<u16> = self.users.keys().copied().collect();
        for uid in &user_ids {
            if let Some(buf) = self.users.get_mut(uid) {
                if let Some(pcm) = buf.next_frame() {
                    let vol = self.volumes.get(uid).copied().unwrap_or(1.0);
                    match &mut mixed {
                        None => {
                            let mut scaled = pcm;
                            if vol != 1.0 {
                                for s in scaled.iter_mut() {
                                    *s *= vol;
                                }
                            }
                            mixed = Some(scaled);
                        }
                        Some(ref mut mix) => {
                            for (i, &s) in pcm.iter().enumerate() {
                                if i < mix.len() {
                                    mix[i] += s * vol;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Clean up idle users
        self.users.retain(|_, buf| !buf.is_idle());

        mixed
    }

    /// Get the set of currently active user IDs.
    pub fn active_users(&self) -> Vec<u16> {
        self.users.keys().copied().collect()
    }
}
