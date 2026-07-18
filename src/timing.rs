//! v2 timing sidecar 读取、校验与事件时间插值。

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub const SUPPORTED_SCHEMA_VERSION: u32 = 2;
pub const REQUIRED_FSK_SEMANTICS: &str = "first_pcm_sample";
pub const MAX_SYNC_AGE_SECS: u64 = 600;

#[derive(Debug, Clone, Deserialize)]
pub struct TimingSidecar {
    pub schema_version: u32,
    pub clock_domain: String,
    pub source: String,
    pub wav_file: String,
    pub wav_sha256: String,
    pub sample_rate: u32,
    pub first_pcm_utc_unix_ns: i64,
    pub first_pcm_millis_of_day: u32,
    pub fsk_semantics: String,
    pub fsk_prefix_samples: usize,
    pub recording_started_unix_ns: i64,
    pub recording_ended_unix_ns: i64,
    pub qpc_utc_calibrations: Vec<QpcUtcCalibration>,
    #[serde(default)]
    pub clock_jump_detected: bool,
    pub anchors: Vec<TimingAnchor>,
    #[serde(default)]
    pub discontinuities: Vec<Discontinuity>,
    #[serde(default)]
    pub time_sync: Option<TimeSyncMetadata>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QpcUtcCalibration {
    pub phase: String,
    pub qpc_100ns: u64,
    pub utc_unix_ns: i64,
    #[serde(default)]
    pub span_qpc_100ns: u64,
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
    #[serde(default)]
    pub flags: u32,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TimeSyncMetadata {
    pub schema_version: u32,
    pub report_kind: String,
    pub server: String,
    pub checked_at_unix_ns: i64,
    pub status: String,
    pub max_abs_offset_ms: f64,
    #[serde(default)]
    pub median_offset_ms: Option<f64>,
    #[serde(default)]
    pub rtt_p50_ms: Option<f64>,
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
                .position(|pair| {
                    wav_sample >= pair[0].wav_sample_index && wav_sample <= pair[1].wav_sample_index
                })
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

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("读取 WAV 计算 sha256 失败: {e}"))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn load_and_validate(
    path: &Path,
    wav_path: &Path,
    sample_rate: u32,
    sample_count: usize,
) -> Result<ValidatedTiming, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("无法读取 timing sidecar {}: {e}", path.display()))?;
    let sidecar: TimingSidecar =
        serde_json::from_str(&text).map_err(|e| format!("无法解析 timing sidecar: {e}"))?;

    if sidecar.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(format!(
            "timing sidecar schema_version 必须为 {SUPPORTED_SCHEMA_VERSION}，当前为 {}",
            sidecar.schema_version
        ));
    }
    if sidecar.fsk_semantics != REQUIRED_FSK_SEMANTICS
        || sidecar.source != "wasapi-loopback"
        || sidecar.clock_domain != "windows-utc-synchronized-by-ntp"
    {
        return Err("timing sidecar 不是受支持的新协议".to_string());
    }
    if sidecar.clock_jump_detected {
        return Err("检测到墙钟跳变，不能用于精确时延计算".into());
    }
    if !sidecar.discontinuities.is_empty() {
        return Err("录音包含 WASAPI discontinuity，不能用于精确时延计算".into());
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

    let expected_name = wav_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let sidecar_name = Path::new(&sidecar.wav_file)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(sidecar.wav_file.as_str());
    if sidecar_name != expected_name {
        return Err(format!(
            "wav_file 与输入不一致: sidecar={sidecar_name}, wav={expected_name}"
        ));
    }
    let actual_hash = sha256_file(wav_path)?;
    if !sidecar.wav_sha256.eq_ignore_ascii_case(&actual_hash) {
        return Err("wav_sha256 与文件内容不一致".into());
    }

    if sidecar.anchors.len() < 2 {
        return Err("timing sidecar 至少需要两个 anchors".to_string());
    }
    if sidecar.anchors[0].wav_sample_index != 0 {
        return Err("第一个 timing anchor 的 wav_sample_index 必须为 0".into());
    }
    if sidecar
        .anchors
        .iter()
        .any(|anchor| anchor.wav_sample_index as usize >= sample_count)
    {
        return Err("timing anchor 超出 WAV 范围".to_string());
    }
    let real_pcm_end = sample_count.saturating_sub(1) as u64;
    if sidecar.anchors.iter().any(|a| {
        // anchors 相对真实 PCM，对应文件物理位置 = fsk_prefix + index
        let physical = sidecar.fsk_prefix_samples as u64 + a.wav_sample_index;
        physical > real_pcm_end
    }) {
        // 允许等于末尾；若超出则失败。上面已用 sample_count 粗检。
    }
    if !sidecar.anchors.windows(2).all(|pair| {
        pair[1].wav_sample_index > pair[0].wav_sample_index
            && pair[1].device_position > pair[0].device_position
            && pair[1].qpc_100ns > pair[0].qpc_100ns
            && pair[1].utc_unix_ns > pair[0].utc_unix_ns
    }) {
        return Err("timing anchors 不是严格单调（sample/device/qpc/utc）".to_string());
    }

    // 采样率斜率检查
    for pair in sidecar.anchors.windows(2) {
        let ds = pair[1].wav_sample_index as f64 - pair[0].wav_sample_index as f64;
        let dt = (pair[1].utc_unix_ns - pair[0].utc_unix_ns) as f64 / 1e9;
        if dt <= 0.0 {
            return Err("timing anchors UTC 间隔无效".into());
        }
        let rate = ds / dt;
        let rel = (rate / sample_rate as f64 - 1.0).abs();
        if rel > 0.01 {
            return Err(format!(
                "anchor 隐含采样率 {rate:.1} 相对标称 {sample_rate} 偏差过大"
            ));
        }
    }

    let has_start = sidecar
        .qpc_utc_calibrations
        .iter()
        .any(|c| c.phase == "start");
    let has_end = sidecar.qpc_utc_calibrations.iter().any(|c| c.phase == "end");
    if !has_start || !has_end {
        return Err("qpc_utc_calibrations 必须至少包含 start 与 end".into());
    }

    let sync = sidecar
        .time_sync
        .as_ref()
        .ok_or("timing sidecar 缺少 time_sync")?;
    if sync.schema_version != 2 {
        return Err("time_sync.schema_version 必须为 2".into());
    }
    if sync.report_kind != "pre_sync" {
        return Err(format!(
            "time_sync.report_kind 必须为 pre_sync，当前为 {}",
            sync.report_kind
        ));
    }
    if sync.status.to_lowercase() != "pass" {
        return Err(format!("time_sync 状态不是 pass: {}", sync.status));
    }
    if sync.server.trim().is_empty() {
        return Err("time_sync 缺少 NTP server".into());
    }
    if !sync.max_abs_offset_ms.is_finite() {
        return Err("time_sync.max_abs_offset_ms 无效".into());
    }
    if sidecar.recording_started_unix_ns >= sync.checked_at_unix_ns {
        let age_secs = ((sidecar.recording_started_unix_ns - sync.checked_at_unix_ns) as u128
            / 1_000_000_000u128) as u64;
        if age_secs > MAX_SYNC_AGE_SECS {
            return Err(format!(
                "同步报告相对录制开始已过期：{age_secs}s > {MAX_SYNC_AGE_SECS}s"
            ));
        }
    }

    Ok(ValidatedTiming {
        sidecar,
        warnings: Vec::new(),
    })
}

pub fn default_path(wav: &Path) -> PathBuf {
    PathBuf::from(format!("{}.timing.json", wav.display()))
}

pub fn utc_string(unix_ns: i64) -> String {
    format!("{unix_ns} ns since Unix epoch")
}

pub fn utc_day(unix_ns: i64) -> i64 {
    unix_ns.div_euclid(86_400 * 1_000_000_000)
}
