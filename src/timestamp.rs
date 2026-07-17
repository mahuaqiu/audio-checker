use std::f64::consts::PI;

pub const FSK_FREQ_0: f64 = 7000.0;
pub const FSK_FREQ_1: f64 = 7500.0;
pub const SYMBOL_DURATION_MS: f64 = 20.0;
pub const PREAMBLE_BITS: usize = 8;
pub const DATA_BITS: usize = 27;
pub const GUARD_DURATION_MS: f64 = 200.0;

#[derive(Debug, Clone, Copy)]
pub struct TimestampMark {
    pub millis_of_day: u32,
    pub marker_samples: usize,
}

pub fn marker_samples(sample_rate: u32) -> usize {
    let symbols = PREAMBLE_BITS + DATA_BITS;
    symbols * (SYMBOL_DURATION_MS * sample_rate as f64 / 1000.0) as usize
        + (GUARD_DURATION_MS * sample_rate as f64 / 1000.0) as usize
}

pub fn decode(samples: &[f32], sample_rate: u32) -> Option<TimestampMark> {
    decode_at(samples, sample_rate, 0)
}

/// 在录音开头的有限范围内搜索 FSK 标记，兼容标记前存在少量保护静音的录音。
pub fn decode_with_offset(samples: &[f32], sample_rate: u32) -> Option<(TimestampMark, usize)> {
    let symbol_samples = (SYMBOL_DURATION_MS * sample_rate as f64 / 1000.0) as usize;
    if symbol_samples == 0 || samples.len() < marker_samples(sample_rate) {
        return None;
    }
    let search_limit = samples
        .len()
        .saturating_sub(marker_samples(sample_rate))
        .min(sample_rate as usize * 2);
    if let Some(mark) = decode_at(samples, sample_rate, 0) {
        return Some((mark, 0));
    }

    let short_window = (sample_rate as usize / 1000).max(1);
    let mut maximum_energy = 0.0f64;
    for start in (0..=search_limit).step_by(short_window) {
        let end = (start + short_window).min(samples.len());
        if end > start {
            let energy = samples[start..end]
                .iter()
                .map(|value| (*value as f64) * (*value as f64))
                .sum::<f64>()
                / (end - start) as f64;
            maximum_energy = maximum_energy.max(energy);
        }
    }
    if maximum_energy <= 1e-12 {
        return None;
    }
    let threshold = maximum_energy * 0.08;
    let estimated = (0..=search_limit).step_by(short_window).find(|start| {
        let end = (*start + short_window).min(samples.len());
        let energy = samples[*start..end]
            .iter()
            .map(|value| (*value as f64) * (*value as f64))
            .sum::<f64>()
            / (end - *start).max(1) as f64;
        energy >= threshold
    })?;
    if let Some(mark) = decode_at(samples, sample_rate, estimated) {
        return Some((mark, estimated));
    }
    let refine_start = estimated.saturating_sub(short_window * 2);
    let refine_end = (estimated + short_window * 2).min(search_limit);
    for candidate in refine_start..=refine_end {
        if let Some(mark) = decode_at(samples, sample_rate, candidate) {
            return Some((mark, candidate));
        }
    }

    let step = (symbol_samples / 4).max(1);
    for candidate in (0..=search_limit).step_by(step) {
        if let Some(mark) = decode_at(samples, sample_rate, candidate) {
            return Some((mark, candidate));
        }
    }
    None
}

fn decode_at(samples: &[f32], sample_rate: u32, offset: usize) -> Option<TimestampMark> {
    decode_at_scored(samples, sample_rate, offset).map(|(mark, _)| mark)
}

fn decode_at_scored(
    samples: &[f32],
    sample_rate: u32,
    offset: usize,
) -> Option<(TimestampMark, f64)> {
    let symbol_samples = (SYMBOL_DURATION_MS * sample_rate as f64 / 1000.0) as usize;
    let total_bits = PREAMBLE_BITS + DATA_BITS;
    let marker_length = marker_samples(sample_rate);
    if symbol_samples == 0 || offset.checked_add(marker_length)? > samples.len() {
        return None;
    }

    let mut bits = Vec::with_capacity(total_bits);
    let mut score = 0.0;
    for bit_index in 0..total_bits {
        let start = offset + bit_index * symbol_samples;
        let window = &samples[start..start + symbol_samples];
        let energy_0 = goertzel(window, FSK_FREQ_0, sample_rate);
        let energy_1 = goertzel(window, FSK_FREQ_1, sample_rate);
        if energy_0.max(energy_1) < 1e-10 {
            return None;
        }
        let bit = if energy_1 > energy_0 { 1u32 } else { 0u32 };
        let expected = if bit_index < PREAMBLE_BITS {
            if bit_index % 2 == 0 {
                1
            } else {
                0
            }
        } else {
            bit
        };
        let magnitude = (energy_1 - energy_0).abs() / (energy_1 + energy_0).max(1e-20);
        score += if expected == 1 { magnitude } else { -magnitude };
        bits.push(bit);
    }

    let hamming = (0..PREAMBLE_BITS)
        .filter(|&i| bits[i] != if i % 2 == 0 { 1 } else { 0 })
        .count();
    if hamming > 1 {
        return None;
    }

    let millis = bits[PREAMBLE_BITS..]
        .iter()
        .fold(0u32, |value, &bit| (value << 1) | bit);
    if millis >= 86_400_000 {
        return None;
    }

    Some((
        TimestampMark {
            millis_of_day: millis,
            marker_samples: marker_length,
        },
        score,
    ))
}

fn goertzel(samples: &[f32], target_freq: f64, sample_rate: u32) -> f64 {
    let n = samples.len() as f64;
    let k = (n * target_freq / sample_rate as f64).round();
    let omega = 2.0 * PI * k / n;
    let coeff = 2.0 * omega.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &sample in samples {
        let s0 = sample as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

pub fn encode_for_test(millis: u32, sample_rate: u32) -> Vec<f32> {
    let symbol_samples = (SYMBOL_DURATION_MS * sample_rate as f64 / 1000.0) as usize;
    let mut bits = Vec::with_capacity(PREAMBLE_BITS + DATA_BITS);
    for i in 0..PREAMBLE_BITS {
        bits.push(if i % 2 == 0 { 1 } else { 0 });
    }
    for i in (0..DATA_BITS).rev() {
        bits.push((millis >> i) & 1);
    }

    let mut result = Vec::with_capacity(marker_samples(sample_rate));
    for bit in bits {
        let frequency = if bit == 1 { FSK_FREQ_1 } else { FSK_FREQ_0 };
        for n in 0..symbol_samples {
            result.push(
                (0.001 * (2.0 * PI * frequency * n as f64 / sample_rate as f64).sin()) as f32,
            );
        }
    }
    result.resize(marker_samples(sample_rate), 0.0);
    result
}

pub fn format_time(millis: f64) -> String {
    let rounded = millis.round().max(0.0) as u64;
    let hours = (rounded / 3_600_000) % 24;
    let minutes = (rounded / 60_000) % 60;
    let seconds = (rounded / 1_000) % 60;
    let milliseconds = rounded % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02}.{milliseconds:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 可以解码两种采样率的时间标记() {
        for sample_rate in [16_000, 48_000] {
            let marker = encode_for_test(36_123_456, sample_rate);
            let decoded = decode(&marker, sample_rate).unwrap();
            assert_eq!(decoded.millis_of_day, 36_123_456);
        }
    }

    #[test]
    fn 可以跳过标记前保护静音() {
        let mut samples = vec![0.0; 1600];
        samples.extend(encode_for_test(1000, 16_000));
        let (decoded, offset) = decode_with_offset(&samples, 16_000).unwrap();
        assert_eq!(decoded.millis_of_day, 1000);
        assert_eq!(offset, 1600);
    }
}
