use crate::domain::BAND_COUNT;

#[derive(Debug, Default)]
pub struct BandAnalyzer;

impl BandAnalyzer {
    pub fn analyze(&self, _samples: &[f32]) -> [f32; BAND_COUNT] {
        [0.0; BAND_COUNT]
    }
}
