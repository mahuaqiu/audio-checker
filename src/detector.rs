//! 自适应拨弦事件候选检测。

#[derive(Debug, Clone, Copy)]
pub struct Event {
    pub onset_sample: usize,
    pub end_sample: usize,
}

const HOP: usize = 16;
const WINDOW: usize = 160;
const MAX_EVENT_FRAMES: usize = 500;

pub fn detect(
    samples: &[f32],
    expected_count: Option<usize>,
    min_gap_ms: f64,
) -> Result<Vec<Event>, String> {
    if samples.len() < WINDOW * 4 {
        return Err("实际录音区太短，无法检测拨弦事件".to_string());
    }
    if !min_gap_ms.is_finite() || min_gap_ms <= 0.0 {
        return Err("事件最小间隔必须大于 0".to_string());
    }

    let samples = crate::preprocess::remove_dc(samples);
    let features = build_features(&samples);
    let envelope: Vec<f32> = features.iter().map(|feature| feature.energy).collect();
    let baseline = percentile(&envelope, 0.20);
    let deviations: Vec<f32> = envelope
        .iter()
        .map(|value| (value - baseline).abs())
        .collect();
    let noise = percentile(&deviations, 0.50).max(1e-6);
    let peak = envelope.iter().copied().fold(0.0f32, f32::max);
    let dynamic_range = (peak - baseline).max(1e-6);
    let energy_threshold = baseline + (noise * 5.0).max(dynamic_range * 0.035);
    let transient_values: Vec<f32> = features.iter().map(|feature| feature.transient).collect();
    let transient_threshold = percentile(&transient_values, 0.75)
        + (percentile(&transient_values, 0.95) - percentile(&transient_values, 0.75)).max(1e-6)
            * 0.25;

    let mut regions = Vec::new();
    let mut active_start = None;
    let mut quiet_frames = 0usize;
    for (index, feature) in features.iter().enumerate() {
        let active = feature.energy > energy_threshold
            || (feature.transient > transient_threshold
                && feature.energy > baseline + dynamic_range * 0.01);
        if active {
            active_start.get_or_insert(index);
            quiet_frames = 0;
        } else if active_start.is_some() {
            quiet_frames += 1;
            if quiet_frames >= 25 {
                let start = active_start.take().unwrap();
                let end = index.saturating_sub(quiet_frames).max(start + 1);
                regions.push((start, end));
                quiet_frames = 0;
            }
        }
    }
    if let Some(start) = active_start {
        regions.push((start, features.len().saturating_sub(1).max(start + 1)));
    }

    let min_gap_frames = min_gap_ms.round().max(1.0) as usize;
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for region in regions {
        if let Some(last) = merged.last_mut() {
            if region.0 < last.0 + min_gap_frames {
                last.1 = region.1;
                continue;
            }
        }
        merged.push(region);
    }

    let mut events = Vec::new();
    for (start_frame, end_frame) in merged {
        let end_frame = end_frame.min(start_frame + MAX_EVENT_FRAMES);
        let start_sample = start_frame * HOP;
        let end_sample = ((end_frame + 1) * HOP).min(samples.len());
        let onset = attack_onset(
            &samples,
            start_sample.saturating_sub(80),
            end_sample,
            baseline,
        );
        let local_peak = features[start_frame..=end_frame.min(features.len() - 1)]
            .iter()
            .map(|feature| feature.energy)
            .fold(0.0, f32::max);
        if local_peak <= baseline + dynamic_range * 0.03 {
            continue;
        }
        events.push(Event {
            onset_sample: onset,
            end_sample,
        });
    }

    if events.is_empty() {
        return Err("未检测到可靠的拨弦事件".to_string());
    }
    if let Some(expected) = expected_count {
        if events.len() != expected {
            return Err(format!(
                "拨弦事件数量不一致：期望 {expected} 次，实际检测到 {} 次",
                events.len()
            ));
        }
    }
    Ok(events)
}

pub fn envelope(samples: &[f32]) -> Vec<f32> {
    build_features(samples)
        .into_iter()
        .map(|feature| feature.energy)
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct Feature {
    energy: f32,
    transient: f32,
}

fn build_features(samples: &[f32]) -> Vec<Feature> {
    let mut result = Vec::new();
    let mut start = 0usize;
    while start < samples.len() {
        let end = (start + WINDOW).min(samples.len());
        let energy = samples[start..end]
            .iter()
            .map(|value| (*value as f64) * (*value as f64))
            .sum::<f64>()
            / (end - start) as f64;
        let transient = samples[start..end]
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs() as f64)
            .sum::<f64>()
            / (end - start).max(2) as f64;
        result.push(Feature {
            energy: energy.sqrt() as f32,
            transient: transient as f32,
        });
        start += HOP;
    }
    result
}

fn attack_onset(samples: &[f32], start: usize, end: usize, baseline: f32) -> usize {
    let end = end.min(samples.len());
    if start >= end {
        return start.min(samples.len().saturating_sub(1));
    }
    let mut short_energy = Vec::new();
    let mut cursor = start;
    while cursor < end {
        let frame_end = (cursor + HOP).min(end);
        let energy = samples[cursor..frame_end]
            .iter()
            .map(|value| (*value as f64) * (*value as f64))
            .sum::<f64>()
            / (frame_end - cursor) as f64;
        short_energy.push((cursor, energy.sqrt() as f32));
        cursor += HOP;
    }
    let peak = short_energy
        .iter()
        .map(|(_, value)| *value)
        .fold(0.0, f32::max);
    let target = baseline + (peak - baseline).max(1e-6) * 0.12;
    short_energy
        .iter()
        .find(|(_, value)| *value >= target)
        .map(|(index, _)| *index)
        .unwrap_or(start)
}

fn percentile(values: &[f32], percentile: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|left, right| left.total_cmp(right));
    let index = ((sorted.len() - 1) as f32 * percentile.clamp(0.0, 1.0)).round() as usize;
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_signal() -> Vec<f32> {
        let mut samples = vec![0.0; 16_000 * 6];
        for &onset in &[16_000usize, 48_000usize, 80_000usize] {
            for index in 0..4_000 {
                let position = onset + index;
                if position < samples.len() {
                    samples[position] += 0.7
                        * (-(index as f32) / 700.0).exp()
                        * (2.0 * std::f32::consts::PI * 420.0 * index as f32 / 16_000.0).sin();
                }
            }
        }
        samples
    }

    #[test]
    fn 可以检测相隔两秒以上的拨弦() {
        let events = detect(&synthetic_signal(), Some(3), 2_000.0).unwrap();
        assert_eq!(events.len(), 3);
        assert!((events[0].onset_sample as isize - 16_000).abs() < 500);
    }
}
