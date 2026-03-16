//! Feedback sounds — short synthesized tones played via cpal.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use log::warn;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Which feedback sound to play.
#[derive(Clone, Copy)]
pub enum Sound {
    /// Rising tone — recording started.
    RecordStart,
    /// Falling tone — recording stopped / transcribing.
    RecordStop,
}

/// Play a short feedback tone on the default output device.
/// Non-blocking: spawns a thread and returns immediately.
pub fn play(sound: Sound) {
    std::thread::spawn(move || {
        if let Err(e) = play_blocking(sound) {
            warn!("feedback sound: {}", e);
        }
    });
}

fn play_blocking(sound: Sound) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let host = crate::audio_toolkit::get_cpal_host();
    let device = host
        .default_output_device()
        .ok_or("no output device")?;

    let config = device.default_output_config()?;
    let sample_rate = config.sample_rate().0 as f32;
    let channels = config.channels() as usize;

    // Generate samples
    let samples = generate(sound, sample_rate);
    let total = samples.len() * channels;
    let cursor = AtomicUsize::new(0);

    let stream = device.build_output_stream(
        &config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            for frame in data.chunks_mut(channels) {
                let pos = cursor.fetch_add(1, Ordering::Relaxed);
                let val = if pos < samples.len() {
                    samples[pos]
                } else {
                    0.0
                };
                for sample in frame.iter_mut() {
                    *sample = val;
                }
            }
        },
        |e| warn!("sound stream error: {}", e),
        None,
    )?;

    stream.play()?;

    // Wait for playback to finish + a small buffer
    let duration_ms = (total as f64 / sample_rate as f64 * 1000.0) as u64 + 30;
    std::thread::sleep(std::time::Duration::from_millis(duration_ms));

    Ok(())
}

fn generate(sound: Sound, sample_rate: f32) -> Vec<f32> {
    match sound {
        Sound::RecordStart => tone_sweep(sample_rate, 600.0, 900.0, 0.12),
        Sound::RecordStop => tone_sweep(sample_rate, 900.0, 600.0, 0.12),
    }
}

/// Generate a sine sweep from `freq_start` to `freq_end` over `duration_secs`
/// with a short fade-in/out to avoid clicks.
fn tone_sweep(sample_rate: f32, freq_start: f32, freq_end: f32, duration_secs: f32) -> Vec<f32> {
    let n = (sample_rate * duration_secs) as usize;
    let fade = (sample_rate * 0.005) as usize; // 5ms fade
    let volume = 0.25_f32;

    let mut samples = Vec::with_capacity(n);
    let mut phase: f32 = 0.0;

    for i in 0..n {
        let t = i as f32 / n as f32;
        let freq = freq_start + (freq_end - freq_start) * t;

        phase += freq / sample_rate;
        let val = (phase * 2.0 * std::f32::consts::PI).sin() * volume;

        // Fade envelope
        let env = if i < fade {
            i as f32 / fade as f32
        } else if i > n - fade {
            (n - i) as f32 / fade as f32
        } else {
            1.0
        };

        samples.push(val * env);
    }

    samples
}
