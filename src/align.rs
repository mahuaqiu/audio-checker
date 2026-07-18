//! 拨弦起音的高精度定位和局部特征相关。

use crate::detector::Event;

const FRAME: usize = 16;
const TEMPLATE_MS: usize = 160;
const SEARCH_MS: usize = 12;
const MIN_CORRELATION: f64 = 0.45;
const MIN_PEAK_MARGIN: f64 = 0.02;
const PEAK_EXCLUSION_MS: usize = SEARCH_MS;
const CANDIDATE_PRIOR_WEIGHT: f64 = 0.10;

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
            )?;
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
) -> Result<usize, String> {
    let template = feature_window(sender_samples, sender_onset, TEMPLATE_MS);
    if template.len() < 8 {
        return Err("发送方拨弦模板过短，无法可靠对齐".to_string());
    }

    let radius = SEARCH_MS * 16;
    let start = receiver_onset.saturating_sub(radius);
    let end = (receiver_onset + radius).min(receiver_samples.len().saturating_sub(1));
    let mut scores = Vec::new();
    let mut candidate = start;
    while candidate <= end {
        let candidate_feature = feature_window(receiver_samples, candidate, TEMPLATE_MS);
        if candidate_feature.len() >= 8 {
            let correlation = normalized_correlation(&template, &candidate_feature);
            if correlation.is_finite() {
                let distance = candidate.abs_diff(receiver_onset) as f64 / radius as f64;
                let score = correlation - CANDIDATE_PRIOR_WEIGHT * distance;
                scores.push((candidate, score, correlation));
            }
        }
        candidate += FRAME;
    }
    let (best, best_score, best_correlation) = scores
        .iter()
        .copied()
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .ok_or_else(|| "接收方拨弦候选过短，无法可靠对齐".to_string())?;

    if best_correlation < MIN_CORRELATION {
        return Err(format!(
            "拨弦模板相关性过低（{best_correlation:.3}），无法可靠定位接收事件"
        ));
    }

    // 只比较局部峰，避免同一相关峰内部的相邻帧被误判为第二个峰。
    let exclusion = PEAK_EXCLUSION_MS * 16;
    let competing_score = scores
        .iter()
        .enumerate()
        .filter(|(index, (candidate, _, _))| {
            candidate.abs_diff(best) > exclusion
                && scores
                    .get(index.saturating_sub(1))
                    .is_none_or(|(_, previous, _)| *previous <= scores[*index].1)
                && scores
                    .get(index + 1)
                    .is_none_or(|(_, next, _)| *next <= scores[*index].1)
        })
        .map(|(_, (_, score, _))| *score)
        .max_by(|left, right| left.total_cmp(right))
        .unwrap_or(f64::NEG_INFINITY);
    if competing_score.is_finite() && best_score - competing_score < MIN_PEAK_MARGIN {
        return Err(format!(
            "拨弦相关峰不够突出（峰值 {:.3}，次峰 {:.3}），无法可靠定位接收事件",
            best_score, competing_score
        ));
    }

    Ok(best)
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

    #[test]
    fn 无相关信号时拒绝强行对齐() {
        let mut sender = vec![0.0f32; 16_000];
        for (index, sample) in sender.iter_mut().enumerate().skip(1_000).take(4_000) {
            *sample = (-(index as f32 - 1_000.0) / 700.0).exp();
        }
        let receiver = vec![0.0f32; 16_000];
        let result = refine_events(
            &sender,
            &receiver,
            &[Event {
                onset_sample: 1_000,
                end_sample: 5_000,
            }],
            &[Event {
                onset_sample: 1_000,
                end_sample: 5_000,
            }],
        );
        assert!(result.is_err());
    }

    #[test]
    fn 精定位不会偏离粗起音太远() {
        let mut sender = vec![0.0f32; 12_000];
        let mut receiver = vec![0.0f32; 12_000];
        for index in 0..3_200 {
            let envelope = (-(index as f32) / 900.0).exp();
            let value = envelope
                * (2.0 * std::f32::consts::PI * 420.0 * index as f32 / 16_000.0).sin();
            sender[1_000 + index] = value;
            receiver[5_000 + index] = value * 0.7;
        }

        let result = correlate_near_candidate(&sender, &receiver, 1_000, 5_000).unwrap();
        assert!(result.abs_diff(5_000) <= SEARCH_MS * FRAME);
    }
}
