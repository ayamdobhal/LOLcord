use anyhow::Result;
use nnnoiseless::DenoiseState;
use audiopus::{
    coder::{Decoder, Encoder},
    packet::Packet,
    Application, Channels, MutSignals, SampleRate,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SupportedStreamConfigRange;
use ringbuf::{
    traits::{Consumer, Producer, Split},
    HeapRb,
};
use shared::voice;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// Thread-safe audio state shared across tokio tasks.
pub struct AudioState {
    pub ptt_active: AtomicBool,
    pub muted: AtomicBool,
    pub deafened: AtomicBool,
    /// If true, transmit always (no PTT needed). If false, PTT mode.
    pub open_mic: AtomicBool,
    /// Whether noise suppression is enabled.
    pub noise_suppression: AtomicBool,
    /// RNNoise denoiser state (expects 480-sample chunks).
    denoiser: Mutex<Box<DenoiseState<'static>>>,
    /// Whether VAD detected voice (for UI indicator).
    pub vad_active: AtomicBool,
    /// VAD threshold (RMS). Range ~0.001 to 0.1. Stored as u32 (value * 10000).
    pub vad_threshold: std::sync::atomic::AtomicU32,
    capture_cons: Mutex<ringbuf::HeapCons<f32>>,
    playback_prod: Mutex<ringbuf::HeapProd<f32>>,
    encoder: Mutex<Encoder>,
    decoder: Mutex<Decoder>,
}

unsafe impl Send for AudioState {}
unsafe impl Sync for AudioState {}

impl AudioState {
    pub fn try_encode_frame(&self) -> Option<Vec<u8>> {
        let is_open_mic = self.open_mic.load(Ordering::Relaxed);
        let is_ptt = self.ptt_active.load(Ordering::Relaxed);
        let is_muted = self.muted.load(Ordering::Relaxed);

        if is_muted || (!is_open_mic && !is_ptt) {
            self.vad_active.store(false, Ordering::Relaxed);
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
            self.vad_active.store(false, Ordering::Relaxed);
            return None;
        }

        // Noise suppression (RNNoise): process in 480-sample chunks
        if self.noise_suppression.load(Ordering::Relaxed) {
            if let Ok(mut denoiser) = self.denoiser.lock() {
                // RNNoise expects 480-sample frames; our frame is 960 samples
                for chunk in pcm.chunks_mut(DenoiseState::FRAME_SIZE) {
                    if chunk.len() == DenoiseState::FRAME_SIZE {
                        let mut buf = [0.0f32; 480];
                        // RNNoise works with values in [-32768, 32767] range
                        for (i, s) in chunk.iter().enumerate() {
                            buf[i] = *s * 32767.0;
                        }
                        let mut out = [0.0f32; 480];
                        denoiser.process_frame(&mut out, &buf);
                        for (i, s) in out.iter().enumerate() {
                            chunk[i] = *s / 32767.0;
                        }
                    }
                }
            }
        }

        // VAD: compute RMS energy
        let rms = {
            let sum_sq: f32 = pcm.iter().map(|&s| s * s).sum();
            (sum_sq / pcm.len() as f32).sqrt()
        };

        let threshold = self.vad_threshold.load(Ordering::Relaxed) as f32 / 10000.0;
        let voice_detected = rms >= threshold;
        self.vad_active.store(voice_detected, Ordering::Relaxed);

        // In open mic mode, gate on VAD. In PTT mode, always transmit.
        if is_open_mic && !voice_detected {
            return None;
        }

        let pcm_i16: Vec<i16> = pcm.iter().map(|&s| (s * 32767.0) as i16).collect();
        let mut opus_buf = vec![0u8; voice::MAX_PACKET_SIZE];

        let len = {
            let encoder = self.encoder.lock().ok()?;
            encoder.encode(&pcm_i16, &mut opus_buf).ok()?
        };
        opus_buf.truncate(len);
        Some(opus_buf)
    }

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

    /// Push already-decoded PCM f32 samples into the playback buffer.
    pub fn push_playback(&self, pcm: &[f32]) {
        if let Ok(mut prod) = self.playback_prod.lock() {
            prod.push_slice(pcm);
        }
    }
}

/// Find best matching stream config from supported ranges.
fn find_best_config(
    configs: impl Iterator<Item = SupportedStreamConfigRange>,
    desired_rate: cpal::SampleRate,
    desired_channels: u16,
) -> Result<cpal::StreamConfig> {
    let mut configs: Vec<_> = configs.collect();

    if configs.is_empty() {
        anyhow::bail!("no supported audio configs");
    }

    // Sort: prefer matching channels, then matching sample rate
    configs.sort_by_key(|c| {
        let ch_diff = (c.channels() as i32 - desired_channels as i32).unsigned_abs();
        let rate_match = if c.min_sample_rate() <= desired_rate && c.max_sample_rate() >= desired_rate {
            0u32
        } else {
            1
        };
        (rate_match, ch_diff)
    });

    let best = &configs[0];
    let sample_rate = if best.min_sample_rate() <= desired_rate && best.max_sample_rate() >= desired_rate
    {
        desired_rate
    } else {
        best.max_sample_rate()
    };

    Ok(cpal::StreamConfig {
        channels: best.channels(),
        sample_rate,
        buffer_size: cpal::BufferSize::Default,
    })
}

/// Downmix multi-channel to mono and resample if needed.
/// Simple linear interpolation resampler.
fn to_mono_48k(data: &[f32], channels: u16, sample_rate: u32) -> Vec<f32> {
    // First: downmix to mono
    let mono: Vec<f32> = if channels == 1 {
        data.to_vec()
    } else {
        data.chunks(channels as usize)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    };

    // Then: resample to 48kHz if needed
    if sample_rate == voice::SAMPLE_RATE {
        return mono;
    }

    let ratio = voice::SAMPLE_RATE as f64 / sample_rate as f64;
    let out_len = (mono.len() as f64 * ratio) as usize;
    let mut out = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_idx = i as f64 / ratio;
        let idx = src_idx as usize;
        let frac = src_idx - idx as f64;

        let s0 = mono.get(idx).copied().unwrap_or(0.0);
        let s1 = mono.get(idx + 1).copied().unwrap_or(s0);
        out.push(s0 + (s1 - s0) * frac as f32);
    }

    out
}

/// Upmix mono to multi-channel and resample from 48kHz if needed.
fn from_mono_48k(data: &[f32], channels: u16, sample_rate: u32) -> Vec<f32> {
    // Resample from 48kHz to device rate
    let resampled = if sample_rate == voice::SAMPLE_RATE {
        data.to_vec()
    } else {
        let ratio = sample_rate as f64 / voice::SAMPLE_RATE as f64;
        let out_len = (data.len() as f64 * ratio) as usize;
        let mut out = Vec::with_capacity(out_len);

        for i in 0..out_len {
            let src_idx = i as f64 / ratio;
            let idx = src_idx as usize;
            let frac = src_idx - idx as f64;

            let s0 = data.get(idx).copied().unwrap_or(0.0);
            let s1 = data.get(idx + 1).copied().unwrap_or(s0);
            out.push(s0 + (s1 - s0) * frac as f32);
        }
        out
    };

    // Upmix to multi-channel
    if channels == 1 {
        resampled
    } else {
        resampled
            .iter()
            .flat_map(|&s| std::iter::repeat_n(s, channels as usize))
            .collect()
    }
}

/// Start audio I/O and return the shared AudioState + streams.
/// Device indices: None = system default, Some(n) = specific device.
pub fn start_audio(
    input_device_idx: Option<usize>,
    output_device_idx: Option<usize>,
) -> Result<(Arc<AudioState>, cpal::Stream, cpal::Stream)> {
    let input_device = crate::devices::get_input_device(input_device_idx)
        .ok_or_else(|| anyhow::anyhow!("input device not found (idx={input_device_idx:?})"))?;
    let output_device = crate::devices::get_output_device(output_device_idx)
        .ok_or_else(|| anyhow::anyhow!("output device not found (idx={output_device_idx:?})"))?;

    tracing::info!("input device: {}", input_device.name().unwrap_or_default());
    tracing::info!("output device: {}", output_device.name().unwrap_or_default());

    let desired_rate = cpal::SampleRate(voice::SAMPLE_RATE);
    let desired_channels = voice::CHANNELS;

    let input_config = {
        let supported = input_device
            .supported_input_configs()
            .map_err(|e| anyhow::anyhow!("input configs: {e}"))?;
        find_best_config(supported, desired_rate, desired_channels)?
    };
    let output_config = {
        let supported = output_device
            .supported_output_configs()
            .map_err(|e| anyhow::anyhow!("output configs: {e}"))?;
        find_best_config(supported, desired_rate, desired_channels)?
    };

    tracing::info!("input config: {:?}", input_config);
    tracing::info!("output config: {:?}", output_config);

    let in_channels = input_config.channels;
    let in_rate = input_config.sample_rate.0;
    let out_channels = output_config.channels;
    let out_rate = output_config.sample_rate.0;

    // Capture ring buffer (mono 48kHz after conversion)
    let capture_rb = HeapRb::<f32>::new(voice::FRAME_SIZE * 10);
    let (capture_prod, capture_cons) = capture_rb.split();
    let capture_prod = Arc::new(Mutex::new(capture_prod));

    // Playback ring buffer (mono 48kHz, converted on output)
    let playback_rb = HeapRb::<f32>::new(voice::FRAME_SIZE * 10);
    let (playback_prod, playback_cons) = playback_rb.split();
    let playback_cons = Arc::new(Mutex::new(playback_cons));

    let encoder = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)?;
    let decoder = Decoder::new(SampleRate::Hz48000, Channels::Mono)?;

    let state = Arc::new(AudioState {
        ptt_active: AtomicBool::new(false),
        muted: AtomicBool::new(false),
        deafened: AtomicBool::new(false),
        open_mic: AtomicBool::new(true), // default: open mic
        noise_suppression: AtomicBool::new(false), // default: off
        denoiser: Mutex::new(DenoiseState::new()),
        vad_active: AtomicBool::new(false),
        vad_threshold: std::sync::atomic::AtomicU32::new(50), // 0.005 RMS default
        capture_cons: Mutex::new(capture_cons),
        playback_prod: Mutex::new(playback_prod),
        encoder: Mutex::new(encoder),
        decoder: Mutex::new(decoder),
    });

    // Input stream — capture, convert to mono 48kHz, push to ring buffer
    let prod = capture_prod.clone();
    let input_frame_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let ifc = input_frame_count.clone();
    let input_stream = input_device.build_input_stream(
        &input_config,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            let mono = to_mono_48k(data, in_channels, in_rate);
            let count = ifc.fetch_add(1, Ordering::Relaxed);
            if count % 500 == 0 {
                // Log every ~10s at 48kHz/20ms
                let max_amp = mono.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                tracing::debug!(
                    "capture: frame={count}, samples={}, max_amp={:.4}",
                    mono.len(),
                    max_amp
                );
            }
            if let Ok(mut prod) = prod.lock() {
                prod.push_slice(&mono);
            }
        },
        |err| tracing::error!("input stream error: {err}"),
        None,
    )?;
    tracing::info!("input stream started (frames logging every ~10s)");

    // Output stream — read mono 48kHz, convert to device format
    let cons = playback_cons.clone();
    let deaf_state = state.clone();
    let output_stream = output_device.build_output_stream(
        &output_config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            if deaf_state.deafened.load(Ordering::Relaxed) {
                data.fill(0.0);
                return;
            }
            // Figure out how many mono 48k samples we need
            let device_frames = data.len() / out_channels as usize;
            let mono_samples = if out_rate == voice::SAMPLE_RATE {
                device_frames
            } else {
                (device_frames as f64 * voice::SAMPLE_RATE as f64 / out_rate as f64).ceil()
                    as usize
            };

            let mut mono = vec![0.0f32; mono_samples];
            if let Ok(mut cons) = cons.lock() {
                for s in mono.iter_mut() {
                    *s = cons.try_pop().unwrap_or(0.0);
                }
            }

            let converted = from_mono_48k(&mono, out_channels, out_rate);
            for (out, &src) in data.iter_mut().zip(converted.iter()) {
                *out = src;
            }
            // Zero any remaining
            if converted.len() < data.len() {
                for s in &mut data[converted.len()..] {
                    *s = 0.0;
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
