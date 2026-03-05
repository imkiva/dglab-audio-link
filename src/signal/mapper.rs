use crate::domain::{
    BAND_COUNT,
    types::{BandRouting, DglabChannel, StrengthRange},
};

pub fn map_band_to_strength(value: f32, threshold: f32, range: StrengthRange) -> u16 {
    let range = range.normalized();
    let value = value.clamp(0.0, 1.0);
    let threshold = threshold.clamp(0.0, 1.0);

    if value <= threshold {
        return 0;
    }

    let span = (range.max - range.min) as f32;
    let normalized = ((value - threshold) / (1.0 - threshold).max(f32::EPSILON)).clamp(0.0, 1.0);
    range.min + (span * normalized).round() as u16
}

pub fn compute_band_outputs(
    values: [f32; BAND_COUNT],
    routing: [BandRouting; BAND_COUNT],
    ranges_by_channel: [StrengthRange; 2],
) -> [Option<(DglabChannel, u16)>; BAND_COUNT] {
    let mut outputs = [None; BAND_COUNT];

    for index in 0..BAND_COUNT {
        let route = routing[index];
        let value = values[index].clamp(0.0, 1.0);
        if route.enabled && value > route.threshold {
            let channel_range = ranges_by_channel[route.channel.index()];
            let strength = map_band_to_strength(value, route.threshold, channel_range);
            outputs[index] = Some((route.channel, strength));
        }
    }

    outputs
}

pub fn aggregate_channel_strengths(
    outputs: [Option<(DglabChannel, u16)>; BAND_COUNT],
) -> [u16; 2] {
    let mut result = [0_u16; 2];
    for output in outputs.into_iter().flatten() {
        match output.0 {
            DglabChannel::A => {
                result[0] = result[0].max(output.1);
            }
            DglabChannel::B => {
                result[1] = result[1].max(output.1);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{aggregate_channel_strengths, compute_band_outputs, map_band_to_strength};
    use crate::domain::types::{BandRouting, DglabChannel, StrengthRange};

    #[test]
    fn returns_zero_when_below_threshold() {
        assert_eq!(
            map_band_to_strength(0.3, 0.5, StrengthRange::new(20, 120)),
            0
        );
    }

    #[test]
    fn reaches_max_when_value_is_one() {
        assert_eq!(
            map_band_to_strength(1.0, 0.4, StrengthRange::new(20, 120)),
            120
        );
    }

    #[test]
    fn disables_non_triggered_bands() {
        let values = [0.9, 0.2, 0.8, 0.7];
        let routing = [
            BandRouting::new(true, 0.5, DglabChannel::A),
            BandRouting::new(true, 0.3, DglabChannel::A),
            BandRouting::new(false, 0.2, DglabChannel::B),
            BandRouting::new(true, 0.6, DglabChannel::B),
        ];

        let outputs = compute_band_outputs(
            values,
            routing,
            [StrengthRange::new(10, 50), StrengthRange::new(10, 50)],
        );
        assert!(outputs[0].is_some());
        assert!(outputs[1].is_none());
        assert!(outputs[2].is_none());
        assert!(outputs[3].is_some());
    }

    #[test]
    fn maps_range_per_channel() {
        let values = [0.95, 0.0, 0.0, 0.95];
        let routing = [
            BandRouting::new(true, 0.1, DglabChannel::A),
            BandRouting::new(false, 0.1, DglabChannel::A),
            BandRouting::new(false, 0.1, DglabChannel::B),
            BandRouting::new(true, 0.1, DglabChannel::B),
        ];

        let outputs = compute_band_outputs(
            values,
            routing,
            [StrengthRange::new(10, 20), StrengthRange::new(80, 100)],
        );
        let a = outputs[0].expect("a output");
        let b = outputs[3].expect("b output");
        assert!(a.1 <= 20);
        assert!(b.1 >= 80);
    }

    #[test]
    fn aggregates_by_channel_with_max_strength() {
        let aggregated = aggregate_channel_strengths([
            Some((DglabChannel::A, 10)),
            Some((DglabChannel::A, 40)),
            Some((DglabChannel::B, 12)),
            Some((DglabChannel::B, 8)),
        ]);
        assert_eq!(aggregated, [40, 12]);
    }
}
