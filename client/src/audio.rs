use anyhow::Result;
use audiopus::{
    coder::{Decoder, Encoder},
    Application, Channels, SampleRate,
    packet::Packet,
    MutSignals,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use shared::voice;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use ringbuf::{HeapRb, traits::{Producer, Consumer, Split}};

/// Thread-safe audio state that can be shared across tokio tasks.
/// The cpal streams live on their own threads and communicate via ring buffers.
pub struct AudioState {
    pub ptt_active: AtomicBool,
    pub muted: AtomicBool,
    pub deafened: AtomicBool,
    /// Capture ring buffer consumer — voice thread reads encoded frames from here
    capture_cons: Mutex<ringbuf::HeapCons<f32>>,
    /// Playback ring buffer producer — voice thread pushes decoded frames here
    playback_prod: Mutex<ringbuf::HeapProd<f32>>,
    /// Opus encoder
    encoder: Mutex<Encoder>,
    /// Opus decoder
    decoder: Mutex<Decoder>,
}

// Safety: The ring buffer halves and Opus coder are behind Mutex.
// cpal streams are NOT in this struct (they live separately).
unsafe impl Send for AudioState {}
unsafe impl Sync for AudioState {}

impl AudioState {
    /// Try to read a frame of captured audio and encode it to Opus.
    pub fn try_encode_frame(&self) -> Option<Vec<u8>> {
        if !self.ptt_active.load(Ordering::Relaxed) || self.muted.load(Ordering::Relaxed) {
            // Drain capture buffer to avoid buildup
            if let Ok(mut cons) = self.capture_cons.lock() {
                let mut discard = [0.0f32; 960];
                while cons.pop_slice(&mut discard) == 960 {}
            }
            return None;
        }

        let mut pcm = vec![0.0f32; voice::FRAME_SIZE];
        let got = {
            let mut cons = self.capture_cons.lock().ok()?;
            cons.pop_slice(&mut pcm)
        };

        if got < voice::FRAME_SIZE {
            return None;
        }

        let pcm_i16: Vec<i16> = pcm.iter().map(|&s| (s * 32767.0) as i16).collect();
        let mut opus_buf = vec![0u8; voice::MAX_PACKET_SIZE];

        let len = {
            let encoder = self.encoder.lock().ok()?;
            encoder
                .encode(&pcm_i16, &mut opus_buf)
                .ok()?
        };
        opus_buf.truncate(len);
        Some(opus_buf)
    }

    /// Decode an Opus frame and push it to the playback buffer.
    pub fn decode_and_play(&self, opus_data: &[u8]) {
        if self.deafened.load(Ordering::Relaxed) {
            return;
        }

        let mut pcm_i16 = vec![0i16; voice::FRAME_SIZE];
        let decoded = {
            let mut decoder = match self.decoder.lock() {
                Ok(d) => d,
                Err(_) => return,
            };
            let packet = match Packet::try_from(opus_data) {
                Ok(p) => p,
                Err(_) => return,
            };
            let output = match MutSignals::try_from(&mut pcm_i16[..]) {
                Ok(o) => o,
                Err(_) => return,
            };
            match decoder.decode(Some(packet), output, false) {
                Ok(n) => n,
                Err(_) => return,
            }
        };

        let pcm_f32: Vec<f32> = pcm_i16[..decoded]
            .iter()
            .map(|&s| s as f32 / 32767.0)
            .collect();

        if let Ok(mut prod) = self.playback_prod.lock() {
            prod.push_slice(&pcm_f32);
        }
    }
}

/// Start audio I/O and return the shared AudioState.
/// The cpal streams are kept alive by returning them (caller must hold them).
pub fn start_audio() -> Result<(Arc<AudioState>, cpal::Stream, cpal::Stream)> {
    let host = cpal::default_host();

    let input_device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no input device"))?;
    let output_device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no output device"))?;

    let config = cpal::StreamConfig {
        channels: voice::CHANNELS,
        sample_rate: cpal::SampleRate(voice::SAMPLE_RATE),
        buffer_size: cpal::BufferSize::Default,
    };

    // Capture ring buffer
    let capture_rb = HeapRb::<f32>::new(voice::FRAME_SIZE * 10);
    let (capture_prod, capture_cons) = capture_rb.split();
    let capture_prod = Arc::new(Mutex::new(capture_prod));

    // Playback ring buffer
    let playback_rb = HeapRb::<f32>::new(voice::FRAME_SIZE * 10);
    let (playback_prod, playback_cons) = playback_rb.split();
    let playback_cons = Arc::new(Mutex::new(playback_cons));

    let encoder = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)?;
    let decoder = Decoder::new(SampleRate::Hz48000, Channels::Mono)?;

    let state = Arc::new(AudioState {
        ptt_active: AtomicBool::new(false),
        muted: AtomicBool::new(false),
        deafened: AtomicBool::new(false),
        capture_cons: Mutex::new(capture_cons),
        playback_prod: Mutex::new(playback_prod),
        encoder: Mutex::new(encoder),
        decoder: Mutex::new(decoder),
    });

    // Input stream
    let prod = capture_prod.clone();
    let input_stream = input_device.build_input_stream(
        &config,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            if let Ok(mut prod) = prod.lock() {
                prod.push_slice(data);
            }
        },
        |err| tracing::error!("input stream error: {err}"),
        None,
    )?;

    // Output stream
    let cons = playback_cons.clone();
    let deaf_state = state.clone();
    let output_stream = output_device.build_output_stream(
        &config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            if deaf_state.deafened.load(Ordering::Relaxed) {
                data.fill(0.0);
                return;
            }
            if let Ok(mut cons) = cons.lock() {
                for sample in data.iter_mut() {
                    *sample = cons.try_pop().unwrap_or(0.0);
                }
            }
        },
        |err| tracing::error!("output stream error: {err}"),
        None,
    )?;

    input_stream.play()?;
    output_stream.play()?;

    Ok((state, input_stream, output_stream))
}
