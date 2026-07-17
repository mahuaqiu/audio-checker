//! 音频分析前的轻量预处理。

/// 去除直流偏移，并限制异常浮点值。
pub fn remove_dc(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let mean = samples
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .sum::<f32>()
        / samples.len() as f32;
    samples
        .iter()
        .map(|value| {
            let value = if value.is_finite() { *value } else { 0.0 };
            (value - mean).clamp(-1.0, 1.0)
        })
        .collect()
}

/// 以 19 阶窗函数 sinc 低通将 48 kHz 降为 16 kHz。
///
/// 输出采样点与输入的三倍采样点对齐，使用对称滤波器避免事件时间整体偏移。
pub fn resample_to_16k(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    if sample_rate == 16_000 {
        return samples.to_vec();
    }
    if sample_rate != 48_000 || samples.is_empty() {
        return Vec::new();
    }

    const TAPS: isize = 19;
    const CUTOFF: f64 = 1.0 / 3.0;
    let output_len = (samples.len() + 2) / 3;
    let mut output = Vec::with_capacity(output_len);
    let half = TAPS / 2;

    for output_index in 0..output_len {
        let center = (output_index * 3) as isize;
        let mut value = 0.0f64;
        let mut weight_sum = 0.0f64;
        for tap in -half..=half {
            let input_index = (center + tap).clamp(0, samples.len() as isize - 1) as usize;
            let distance = tap as f64;
            let sinc = if distance == 0.0 {
                1.0
            } else {
                let x = std::f64::consts::PI * distance * CUTOFF;
                x.sin() / x
            };
            let window =
                0.5 + 0.5 * (std::f64::consts::PI * (tap + half) as f64 / (TAPS - 1) as f64).cos();
            let weight = CUTOFF * sinc * window;
            value += samples[input_index] as f64 * weight;
            weight_sum += weight;
        }
        output.push(if weight_sum.abs() > f64::EPSILON {
            (value / weight_sum) as f32
        } else {
            0.0
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 去直流后均值接近零() {
        let result = remove_dc(&[0.2, 0.3, 0.4]);
        assert!(result.iter().sum::<f32>().abs() < 1e-6);
    }

    #[test]
    fn 重采样保持长度比例() {
        assert_eq!(resample_to_16k(&vec![0.0; 48_000], 48_000).len(), 16_000);
    }
}
