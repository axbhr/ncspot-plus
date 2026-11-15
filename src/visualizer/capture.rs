//! Copies the played audio from the librespot sink for the visualizer.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use librespot_playback::audio_backend::{Sink, SinkBuilder, SinkResult};
use librespot_playback::config::AudioFormat;
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;

pub const SAMPLE_RATE: f64 = 44100.0;
/// Captured history, long enough to cover the output buffer latency.
const MAX_FRAMES: usize = 5 * 44100;

struct Capture {
    frames: VecDeque<[f32; 2]>,
    written: u64,
    started: Option<Instant>,
}

static CAPTURE: Mutex<Capture> = Mutex::new(Capture {
    frames: VecDeque::new(),
    written: 0,
    started: None,
});
static INNER: OnceLock<SinkBuilder> = OnceLock::new();

/// Wrap `builder` so every played sample is also captured.
pub fn tap(builder: SinkBuilder) -> SinkBuilder {
    let _ = INNER.set(builder);
    tapped_sink
}

fn tapped_sink(device: Option<String>, format: AudioFormat) -> Box<dyn Sink> {
    let inner = INNER.get().expect("tap() stores the inner sink builder");
    Box::new(TapSink(inner(device, format)))
}

fn reset() {
    let mut capture = CAPTURE.lock().unwrap();
    capture.frames.clear();
    capture.written = 0;
    capture.started = None;
}

struct TapSink(Box<dyn Sink>);

impl Sink for TapSink {
    fn start(&mut self) -> SinkResult<()> {
        reset();
        self.0.start()
    }

    fn stop(&mut self) -> SinkResult<()> {
        reset();
        self.0.stop()
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        if let AudioPacket::Samples(samples) = &packet {
            let mut capture = CAPTURE.lock().unwrap();
            capture.started.get_or_insert_with(Instant::now);
            for frame in samples.as_chunks::<2>().0 {
                capture.frames.push_back([frame[0] as f32, frame[1] as f32]);
            }
            capture.written += (samples.len() / 2) as u64;
            let excess = capture.frames.len().saturating_sub(MAX_FRAMES);
            capture.frames.drain(..excess);
        }
        self.0.write(packet, converter)
    }
}

/// The `n` stereo frames audible right now, skipping what is still buffered
/// in the audio output.
pub fn audible_window(n: usize) -> Option<Vec<[f32; 2]>> {
    let capture = CAPTURE.lock().unwrap();
    let started = capture.started?;
    let played = ((started.elapsed().as_secs_f64() * SAMPLE_RATE) as u64).min(capture.written);
    let buffered = (capture.written - played) as usize;
    let end = capture.frames.len().checked_sub(buffered)?;
    let start = end.checked_sub(n)?;
    Some(capture.frames.range(start..end).copied().collect())
}
