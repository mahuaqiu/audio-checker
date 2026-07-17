//! 事件配对、时延校验和统计。

#[derive(Debug, Clone, Copy)]
pub struct LatencySummary {
    pub median_ms: f64,
    pub average_ms: f64,
    pub minimum_ms: f64,
    pub maximum_ms: f64,
}

pub fn summarize(
    latencies: &[f64],
    max_latency_ms: f64,
) -> Result<LatencySummary, (String, String)> {
    if latencies.is_empty() {
        return Err((
            "EVENT_DETECTION_UNCERTAIN".to_string(),
            "没有可用于计算时延的拨弦事件".to_string(),
        ));
    }
    if let Some((index, latency)) = latencies
        .iter()
        .enumerate()
        .find(|(_, value)| !value.is_finite() || **value < 0.0 || **value > max_latency_ms)
    {
        return Err((
            "LATENCY_OUT_OF_RANGE".to_string(),
            format!(
                "第 {} 次拨弦时延为 {:.3} 毫秒，合理范围为 0 至 {:.0} 毫秒",
                index + 1,
                latency,
                max_latency_ms
            ),
        ));
    }

    let mut sorted = latencies.to_vec();
    sorted.sort_by(|left, right| left.total_cmp(right));
    let median_ms = if sorted.len() % 2 == 0 {
        (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) / 2.0
    } else {
        sorted[sorted.len() / 2]
    };
    let average_ms = latencies.iter().sum::<f64>() / latencies.len() as f64;
    let minimum_ms = sorted[0];
    let maximum_ms = *sorted.last().unwrap();

    if latencies.len() >= 2 && maximum_ms - minimum_ms > 100.0 {
        return Err((
            "LATENCY_UNSTABLE".to_string(),
            format!(
                "多次拨弦时延波动 {:.3} 毫秒，超过允许的 100 毫秒",
                maximum_ms - minimum_ms
            ),
        ));
    }

    Ok(LatencySummary {
        median_ms,
        average_ms,
        minimum_ms,
        maximum_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 统计结果使用中位数() {
        let result = summarize(&[247.0, 245.0, 249.0], 500.0).unwrap();
        assert_eq!(result.median_ms, 247.0);
        assert_eq!(result.minimum_ms, 245.0);
        assert_eq!(result.maximum_ms, 249.0);
    }

    #[test]
    fn 超出范围返回结构化错误() {
        let result = summarize(&[610.0], 500.0).unwrap_err();
        assert_eq!(result.0, "LATENCY_OUT_OF_RANGE");
    }
}
