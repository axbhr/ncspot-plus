//! Spectrum analysis and drawing of the cover view visualizer.

mod capture;
pub mod colors;
mod styles;

pub use capture::tap;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cursive::Printer;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::settings::{CoverSettings, Decay, Visual};
use capture::{SAMPLE_RATE, audible_window};
use colors::Paint;

const FFT_SIZE: usize = 2048;
const MIN_FREQ: f32 = 40.0;
const MAX_FREQ: f32 = 16000.0;
const CIRCLE_BANDS: usize = 32;
/// Level meter range in dB below full scale.
const METER_RANGE_DB: f32 = 48.0;

/// Analysis results the styles draw from, all values in 0..1.
#[derive(Default)]
pub struct Frame {
    pub bands: Vec<f32>,
    pub bands_right: Vec<f32>,
    pub peaks: Vec<f32>,
    pub peaks_right: Vec<f32>,
    pub wave: Vec<f32>,
    /// Past `bands`, oldest first.
    pub history: VecDeque<Vec<f32>>,
    pub levels: [f32; 2],
    pub level_peaks: [f32; 2],
}

struct State {
    frame: Frame,
    reference_db: f32,
}

pub struct Visualizer {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    state: Mutex<State>,
}

impl Default for Visualizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Visualizer {
    pub fn new() -> Self {
        let window = (0..FFT_SIZE)
            .map(|i| {
                let x = i as f32 / (FFT_SIZE - 1) as f32;
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * x).cos()
            })
            .collect();
        Self {
            fft: FftPlanner::new().plan_fft_forward(FFT_SIZE),
            window,
            state: Mutex::new(State {
                frame: Frame::default(),
                reference_db: -90.0,
            }),
        }
    }

    /// True once everything has fallen back to silence.
    pub fn is_idle(&self) -> bool {
        let state = self.state.lock().unwrap();
        let frame = &state.frame;
        let quiet = |values: &[f32]| values.iter().all(|v| *v < 0.01);
        quiet(&frame.bands)
            && quiet(&frame.bands_right)
            && quiet(&frame.peaks)
            && quiet(&frame.levels)
            && quiet(&frame.level_peaks)
            && frame.history.iter().all(|row| quiet(row))
    }

    pub fn draw(&self, printer: &Printer<'_, '_>, settings: &CoverSettings, paint: &Paint) {
        let (width, height) = (printer.size.x, printer.size.y);
        if settings.visualizer == Visual::Off || width == 0 || height == 0 {
            return;
        }
        self.update(band_count(settings, width), settings, height * 2);
        let state = self.state.lock().unwrap();
        styles::draw(printer, &state.frame, settings, paint);
    }

    /// Spectrum of `samples` in dB for `bands` log-spaced bands.
    fn spectrum(&self, samples: &[f32], bands: usize) -> Vec<f32> {
        let mut buffer: Vec<Complex<f32>> = samples
            .iter()
            .zip(&self.window)
            .map(|(s, w)| Complex::new(s * w, 0.0))
            .collect();
        self.fft.process(&mut buffer);

        let bin_hz = SAMPLE_RATE as f32 / FFT_SIZE as f32;
        let ratio = MAX_FREQ / MIN_FREQ;
        (0..bands)
            .map(|i| {
                let lo = MIN_FREQ * ratio.powf(i as f32 / bands as f32);
                let hi = MIN_FREQ * ratio.powf((i + 1) as f32 / bands as f32);
                let lo_bin = ((lo / bin_hz) as usize).max(1);
                let hi_bin = ((hi / bin_hz) as usize).max(lo_bin + 1).min(FFT_SIZE / 2);
                let magnitude = buffer[lo_bin..hi_bin]
                    .iter()
                    .map(|c| c.norm())
                    .fold(0.0, f32::max);
                // Tilt by +3 dB per octave so highs are not drowned out.
                let tilt = 3.0 * (lo / MIN_FREQ).log2();
                20.0 * (magnitude / FFT_SIZE as f32).max(1e-9).log10() + tilt
            })
            .collect()
    }

    fn update(&self, bands: usize, settings: &CoverSettings, history_rows: usize) {
        let mut state = self.state.lock().unwrap();
        let State {
            frame,
            reference_db,
        } = &mut *state;
        let stereo = settings.visualizer == Visual::Stereo;
        let right_bands = if stereo { bands } else { 0 };
        frame.bands.resize(bands, 0.0);
        frame.peaks.resize(bands, 0.0);
        frame.bands_right.resize(right_bands, 0.0);
        frame.peaks_right.resize(right_bands, 0.0);
        if frame.history.front().is_some_and(|row| row.len() != bands) {
            frame.history.clear();
        }

        let (decay, fall) = match settings.decay {
            Decay::Slow => (0.9, 0.008),
            Decay::Normal => (0.82, 0.015),
            Decay::Fast => (0.65, 0.03),
        };
        let range = settings.sensitivity.max(10) as f32;

        let mut left = vec![0.0; bands];
        let mut right = vec![0.0; right_bands];
        let mut levels = [0.0; 2];
        match audible_window(FFT_SIZE) {
            Some(frames) => {
                let channel = |c: usize| frames.iter().map(|f| f[c]).collect::<Vec<f32>>();
                let mono: Vec<f32> = frames.iter().map(|f| (f[0] + f[1]) * 0.5).collect();
                let spectra = if stereo {
                    vec![
                        self.spectrum(&channel(0), bands),
                        self.spectrum(&channel(1), bands),
                    ]
                } else {
                    vec![self.spectrum(&mono, bands)]
                };
                let loudest = spectra.iter().flatten().copied().fold(f32::MIN, f32::max);
                *reference_db = (*reference_db - 0.15).max(loudest);
                let floor = *reference_db - range;
                let normalize = |db: &f32| ((db - floor) / range).clamp(0.0, 1.0);
                left = spectra[0].iter().map(normalize).collect();
                if let Some(spectrum) = spectra.get(1) {
                    right = spectrum.iter().map(normalize).collect();
                }

                for (c, level) in levels.iter_mut().enumerate() {
                    let rms = (frames.iter().map(|f| f[c] * f[c]).sum::<f32>()
                        / frames.len() as f32)
                        .sqrt();
                    let db = 20.0 * rms.max(1e-9).log10();
                    *level = ((db + METER_RANGE_DB) / METER_RANGE_DB).clamp(0.0, 1.0);
                }

                let gain = mono.iter().map(|s| s.abs()).fold(0.0, f32::max).max(1e-3);
                frame.wave = mono[FFT_SIZE / 2..].iter().map(|s| s / gain).collect();
            }
            None => frame.wave.clear(),
        }

        smooth(&mut frame.bands, &mut frame.peaks, &left, decay, fall);
        smooth(
            &mut frame.bands_right,
            &mut frame.peaks_right,
            &right,
            decay,
            fall,
        );
        smooth(
            &mut frame.levels,
            &mut frame.level_peaks,
            &levels,
            decay,
            fall,
        );

        frame.history.push_back(frame.bands.clone());
        while frame.history.len() > history_rows {
            frame.history.pop_front();
        }
    }
}

/// Fast attack, configurable release, slowly falling peaks.
fn smooth(values: &mut [f32], peaks: &mut [f32], targets: &[f32], decay: f32, fall: f32) {
    for ((value, peak), target) in values.iter_mut().zip(peaks.iter_mut()).zip(targets) {
        *value = if *target > *value {
            *value * 0.3 + target * 0.7
        } else {
            *value * decay + target * (1.0 - decay)
        };
        *peak = (*peak - fall).max(*value);
    }
}

/// Number of frequency bands a style needs for `width` columns.
fn band_count(settings: &CoverSettings, width: usize) -> usize {
    let gap = settings.bar_gap as usize;
    let stride = settings.bar_width.max(1) + gap;
    let count = match settings.visualizer {
        Visual::Bars | Visual::Mirror | Visual::Stereo => (width + gap) / stride,
        Visual::Dots => (width * 2 + gap) / stride,
        Visual::Line | Visual::Waterfall => width,
        Visual::Circle => CIRCLE_BANDS,
        Visual::Meter | Visual::Wave | Visual::Off => 1,
    };
    count.max(1)
}
