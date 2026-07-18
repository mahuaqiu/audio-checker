use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;
pub const REQUIRED_FSK_SEMANTICS: &str = "first_pcm_sample";

#[derive(Debug, Clone, Deserialize)]
pub struct TimingSidecar {
    pub schema_version: u32,
    pub clock_domain: String,
    pub source: String,
    pub sample_rate: u32,
    pub first_pcm_utc_unix_ns: i64,
    pub first_pcm_millis_of_day: u32,
    pub fsk_semantics: String,
    pub fsk_prefix_samples: usize,
    pub anchors: Vec<TimingAnchor>,
    #[serde(default)]
    pub discontinuities: Vec<Discontinuity>,
    #[serde(default)]
    pub time_sync: Option<TimeSyncMetadata>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TimingAnchor {
    pub wav_sample_index: u64,
    pub device_position: u64,
    pub qpc_100ns: u64,
    pub utc_unix_ns: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Discontinuity {
    pub wav_sample_index: u64,
    pub flags: u32,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TimeSyncMetadata {
    pub server: Option<String>,
    pub status: String,
    pub max_abs_offset_ms: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ValidatedTiming {
    pub sidecar: TimingSidecar,
    pub warnings: Vec<String>,
}

impl ValidatedTiming {
    pub fn event_utc_ns(&self, wav_sample: u64) -> Result<i64, String> {
        let anchors = &self.sidecar.anchors;
        let pair = if wav_sample <= anchors[0].wav_sample_index {
            (&anchors[0], &anchors[1])
        } else if wav_sample >= anchors[anchors.len() - 1].wav_sample_index {
            (&anchors[anchors.len() - 2], &anchors[anchors.len() - 1])
        } else {
            let index = anchors
                .windows(2)
                .position(|pair| wav_sample >= pair[0].wav_sample_index && wav_sample <= pair[1].wav_sample_index)
                .ok_or_else(|| "找不到事件对应的 timing anchor 区间".to_string())?;
            (&anchors[index], &anchors[index + 1])
        };
        interpolate(pair.0, pair.1, wav_sample)
    }
}

fn interpolate(left: &TimingAnchor, right: &TimingAnchor, sample: u64) -> Result<i64, String> {
    let sample_delta = right.wav_sample_index as i128 - left.wav_sample_index as i128;
    let utc_delta = right.utc_unix_ns as i128 - left.utc_unix_ns as i128;
    if sample_delta <= 0 || utc_delta <= 0 {
        return Err("timing anchors 不满足严格单调".to_string());
    }
    let value = left.utc_unix_ns as i128
        + (sample as i128 - left.wav_sample_index as i128) * utc_delta / sample_delta;
    i64::try_from(value).map_err(|_| "事件 UTC 纳秒超出整数范围".to_string())
}

pub fn load_and_validate(
    path: &Path,
    sample_rate: u32,
    sample_count: usize,
) -> Result<ValidatedTiming, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("无法读取 timing sidecar {}: {e}", path.display()))?;
    let sidecar: TimingSidecar = serde_json::from_str(&text)
        .map_err(|e| format!("无法解析 timing sidecar: {e}"))?;
    if sidecar.schema_version != SUPPORTED_SCHEMA_VERSION
        || sidecar.fsk_semantics != REQUIRED_FSK_SEMANTICS
        || sidecar.source != "wasapi-loopback"
        || sidecar.clock_domain != "windows-utc-synchronized-by-ntp"
    {
        return Err("timing sidecar 不是受支持的新协议".to_string());
    }
    if sidecar.sample_rate != sample_rate {
        return Err(format!(
            "WAV 采样率 {sample_rate} 与 sidecar 采样率 {} 不一致",
            sidecar.sample_rate
        ));
    }
    if sidecar.fsk_prefix_samples == 0 || sidecar.fsk_prefix_samples >= sample_count {
        return Err("timing sidecar 的 FSK 前缀范围无效".to_string());
    }
    if sidecar.anchors.len() < 2 {
        return Err("timing sidecar 至少需要两个 anchors".to_string());
    }
    if sidecar.anchors.iter().any(|anchor| anchor.wav_sample_index as usize >= sample_count) {
        return Err("timing anchor 超出 WAV 范围".to_string());
    }
    if !sidecar.anchors.windows(2).all(|pair| {
        pair[1].wav_sample_index > pair[0].wav_sample_index
            && pair[1].utc_unix_ns > pair[0].utc_unix_ns
    }) {
        return Err("timing anchors 不是严格单调".to_string());
    }
    if !sidecar.discontinuities.is_empty() {
        return Err("录音包含 WASAPI discontinuity，不能用于精确时延计算".to_string());
    }
    let sync = sidecar
        .time_sync
        .as_ref()
        .ok_or("timing sidecar 缺少 time_sync")?;
    if sync.status.to_lowercase() != "pass" || sync.server.as_deref().unwrap_or("").is_empty() {
        return Err("timing sidecar 的 time_sync 未通过或缺少 NTP server".to_string());
    }
    Ok(ValidatedTiming { sidecar, warnings: Vec::new() })
}

pub fn default_path(wav: &Path) -> PathBuf {
    PathBuf::from(format!("{}.timing.json", wav.display()))
}

pub fn utc_string(unix_ns: i64) -> String {
    format!("{unix_ns} ns since Unix epoch")
}
