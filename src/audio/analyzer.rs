use rustfft::{Fft, FftPlanner, num_complex::Complex32};

use crate::types::{BAND_COUNT, BAND_PROFILES};

#[derive(Debug, Clone, Copy, Default)]
pub struct BandAnalysisFrame {
    pub energy: [f32; BAND_COUNT],
    pub onset: [f32; BAND_COUNT],
}

pub struct BandAnalyzer {
    sample_rate: f32,
    frame_size: usize,
    window: Vec<f32>,
    fft: std::sync::Arc<dyn Fft<f32>>,
    spectrum: Vec<Complex32>,
    band_peaks: [f32; BAND_COUNT],
    previous_band_energy: [f32; BAND_COUNT],
    onset_peaks: [f32; BAND_COUNT],
}

impl BandAnalyzer {
    pub fn new(sample_rate: u32, frame_size: usize) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(frame_size);
        let window = hann_window(frame_size);
        let spectrum = vec![Complex32::new(0.0, 0.0); frame_size];
        Self {
            sample_rate: sample_rate as f32,
            frame_size,
            window,
            fft,
            spectrum,
            band_peaks: [1e-6; BAND_COUNT],
            previous_band_energy: [0.0; BAND_COUNT],
            onset_peaks: [1e-6; BAND_COUNT],
        }
    }

    pub fn analyze(&mut self, samples: &[f32]) -> BandAnalysisFrame {
        if self.frame_size == 0 {
            return BandAnalysisFrame::default();
        }

        let mut frame = vec![0.0_f32; self.frame_size];
        if samples.len() >= self.frame_size {
            frame.copy_from_slice(&samples[..self.frame_size]);
        } else {
            frame[..samples.len()].copy_from_slice(samples);
        }

        for (idx, value) in frame.iter().enumerate() {
            self.spectrum[idx] = Complex32::new(value * self.window[idx], 0.0);
        }

        self.fft.process(&mut self.spectrum);

        let mut energies = [0.0_f32; BAND_COUNT];
        let mut weights = [0.0_f32; BAND_COUNT];
        let nyquist_bins = self.frame_size / 2;

        for bin in 1..nyquist_bins {
            let freq = bin as f32 * self.sample_rate / self.frame_size as f32;
            let mag = self.spectrum[bin].norm_sqr();
            for band_idx in 0..BAND_COUNT {
                let profile = BAND_PROFILES[band_idx];
                let weight =
                    band_weight_log(freq, profile.low_hz, profile.focus_hz, profile.high_hz);
                if weight > 0.0 {
                    energies[band_idx] += mag * weight;
                    weights[band_idx] += weight;
                }
            }
        }

        let mut normalized = [0.0_f32; BAND_COUNT];
        let mut onset = [0.0_f32; BAND_COUNT];
        for band_idx in 0..BAND_COUNT {
            let energy = if weights[band_idx] > 0.0 {
                (energies[band_idx] / weights[band_idx]).sqrt()
            } else {
                0.0
            };

            self.band_peaks[band_idx] = self.band_peaks[band_idx]
                .mul_add(0.98, 0.0)
                .max(energy)
                .max(1e-6);
            normalized[band_idx] = (energy / self.band_peaks[band_idx]).clamp(0.0, 1.0);

            let previous_energy = self.previous_band_energy[band_idx];
            let onset_energy = (energy - previous_energy).max(0.0);
            self.previous_band_energy[band_idx] = previous_energy.mul_add(0.72, energy * 0.28);
            self.onset_peaks[band_idx] = self.onset_peaks[band_idx]
                .mul_add(0.94, 0.0)
                .max(onset_energy)
                .max(1e-6);
            onset[band_idx] = (onset_energy / self.onset_peaks[band_idx]).clamp(0.0, 1.0);
        }

        BandAnalysisFrame {
            energy: normalized,
            onset,
        }
    }
}

impl Default for BandAnalyzer {
    fn default() -> Self {
        Self::new(48_000, 1_024)
    }
}

fn band_weight_log(freq: f32, low_hz: f32, focus_hz: f32, high_hz: f32) -> f32 {
    if !(low_hz > 0.0 && low_hz < focus_hz && focus_hz < high_hz) {
        return 0.0;
    }
    if freq < low_hz || freq > high_hz {
        return 0.0;
    }

    let freq = freq.log10();
    let low_hz = low_hz.log10();
    let focus_hz = focus_hz.log10();
    let high_hz = high_hz.log10();

    if freq <= focus_hz {
        ((freq - low_hz) / (focus_hz - low_hz)).clamp(0.0, 1.0)
    } else {
        ((high_hz - freq) / (high_hz - focus_hz)).clamp(0.0, 1.0)
    }
}

fn hann_window(size: usize) -> Vec<f32> {
    if size <= 1 {
        return vec![1.0; size];
    }
    let denom = (size - 1) as f32;
    (0..size)
        .map(|n| {
            let phase = 2.0_f32 * std::f32::consts::PI * n as f32 / denom;
            0.5 - 0.5 * phase.cos()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::BandAnalyzer;

    #[test]
    fn responds_to_bass_sine_in_expected_band() {
        let mut analyzer = BandAnalyzer::new(48_000, 1_024);
        let freq = 200.0_f32;
        let samples: Vec<f32> = (0..1_024)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect();
        let bands = analyzer.analyze(&samples);
        assert!(bands.energy[1] >= bands.energy[0]);
        assert!(bands.energy[1] >= bands.energy[2]);
    }

    #[test]
    fn responds_to_voice_sine_in_expected_band() {
        let mut analyzer = BandAnalyzer::new(48_000, 1_024);
        let freq = 900.0_f32;
        let samples: Vec<f32> = (0..1_024)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect();
        let bands = analyzer.analyze(&samples);
        assert!(bands.energy[2] >= bands.energy[1]);
        assert!(bands.energy[2] >= bands.energy[3]);
    }

    #[test]
    fn responds_to_hihat_like_sine_in_expected_band() {
        let mut analyzer = BandAnalyzer::new(48_000, 1_024);
        let freq = 7_000.0_f32;
        let samples: Vec<f32> = (0..1_024)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect();
        let bands = analyzer.analyze(&samples);
        assert!(bands.energy[3] >= bands.energy[2]);
        assert!(bands.energy[3] >= bands.energy[1]);
    }

    #[test]
    fn onset_is_higher_on_attack_than_on_sustain() {
        let mut analyzer = BandAnalyzer::new(48_000, 1_024);
        let attack_samples: Vec<f32> = (0..1_024)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                (2.0 * std::f32::consts::PI * 80.0 * t).sin() * 0.9
            })
            .collect();
        let attack = analyzer.analyze(&attack_samples);
        let sustain = analyzer.analyze(&attack_samples);
        assert!(attack.onset[0] >= sustain.onset[0]);
    }
}
