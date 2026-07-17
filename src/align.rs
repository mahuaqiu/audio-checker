//! 拨弦起音的高精度定位和局部特征相关。

use crate::detector::Event;

const FRAME: usize = 16;
const TEMPLATE_MS: usize = 260;
const SEARCH_MS: usize = 45;

pub fn refine_events(
    sender_samples: &[f32],
    receiver_samples: &[f32],
    sender_events: &[Event],
    receiver_events: &[Event],
) -> Result<Vec<(usize, usize)>, String> {
    if sender_events.len() != receiver_events.len() {
        return Err("发送方和接收方事件数量不一致，无法进行对齐".to_string());
    }

    sender_events
        .iter()
        .zip(receiver_events)
        .map(|(sender_event, receiver_event)| {
            let sender_onset = refine_onset(sender_samples, *sender_event);
            let receiver_onset = refine_onset(receiver_samples, *receiver_event);
            let receiver_onset = correlate_near_candidate(
                sender_samples,
                receiver_samples,
                sender_onset,
                receiver_onset,
            );
            Ok((sender_onset, receiver_onset))
        })
        .collect()
}

pub fn refine_onset(samples: &[f32], event: Event) -> usize {
    if samples.is_empty() {
        return 0;
    }
    let start = event.onset_sample.saturating_sub(80);
    let end = event.end_sample.min(samples.len()).max(start + 1);
    let mut frame_energy = Vec::new();
    let mut frame_start = start;
    while frame_start < end {
        let frame_end = (frame_start + FRAME).min(end);
        let energy = samples[frame_start..frame_end]
            .iter()
            .map(|value| (*value as f64) * (*value as f64))
            .sum::<f64>()
            / (frame_end - frame_start) as f64;
        frame_energy.push((frame_start, energy.sqrt() as f32));
        frame_start += FRAME;
    }
    if frame_energy.len() < 2 {
        return event.onset_sample.min(samples.len().saturating_sub(1));
    }

    let peak = frame_energy
        .iter()
        .map(|(_, value)| *value)
        .fold(0.0, f32::max);
    let baseline = frame_energy
        .iter()
        .take(3)
        .map(|(_, value)| *value)
        .fold(f32::INFINITY, f32::min)
        .min(peak);
    let target = baseline + (peak - baseline).max(1e-6) * 0.12;
    frame_energy
        .iter()
        .find(|(_, value)| *value >= target)
        .map(|(index, _)| *index)
        .unwrap_or(event.onset_sample)
        .min(samples.len().saturating_sub(1))
}

fn correlate_near_candidate(
    sender_samples: &[f32],
    receiver_samples: &[f32],
    sender_onset: usize,
    receiver_onset: usize,
) -> usize {
    let template = feature_window(sender_samples, sender_onset, TEMPLATE_MS);
    if template.len() < 8 {
        return receiver_onset;
    }

    let radius = SEARCH_MS * 16;
    let start = receiver_onset.saturating_sub(radius);
    let end = (receiver_onset + radius).min(receiver_samples.len().saturating_sub(1));
    let mut best = receiver_onset;
    let mut best_score = f64::NEG_INFINITY;
    let mut candidate = start;
    while candidate <= end {
        let candidate_feature = feature_window(receiver_samples, candidate, TEMPLATE_MS);
        let score = normalized_correlation(&template, &candidate_feature);
        if score > best_score {
            best_score = score;
            best = candidate;
        }
        candidate += FRAME;
    }
    best
}

fn feature_window(samples: &[f32], onset: usize, duration_ms: usize) -> Vec<f32> {
    let frame_count = duration_ms * 16;
    let mut result = Vec::with_capacity(frame_count);
    for frame in 0..frame_count {
        let start = onset.saturating_add(frame * FRAME);
        if start >= samples.len() {
            break;
        }
        let end = (start + FRAME).min(samples.len());
        let energy = samples[start..end]
            .iter()
            .map(|value| (*value as f64) * (*value as f64))
            .sum::<f64>()
            / (end - start) as f64;
        result.push((energy.sqrt() as f32).ln_1p());
    }
    result
}

fn normalized_correlation(left: &[f32], right: &[f32]) -> f64 {
    let length = left.len().min(right.len());
    if length < 8 {
        return f64::NEG_INFINITY;
    }
    let left_mean = left[..length].iter().sum::<f32>() / length as f32;
    let right_mean = right[..length].iter().sum::<f32>() / length as f32;
    let mut numerator = 0.0f64;
    let mut left_power = 0.0f64;
    let mut right_power = 0.0f64;
    for index in 0..length {
        let l = (left[index] - left_mean) as f64;
        let r = (right[index] - right_mean) as f64;
        numerator += l * r;
        left_power += l * l;
        right_power += r * r;
    }
    numerator / (left_power.sqrt() * right_power.sqrt()).max(1e-12)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 相关函数能识别相同形状() {
        assert!(
            normalized_correlation(
                &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
                &[2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0],
            ) > 0.99
        );
    }
}
