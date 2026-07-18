use audio_checker::{analyze_paths, timestamp, AnalysisOptions};
use hound::{SampleFormat, WavSpec, WavWriter};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

fn temporary_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("audio-checker-{name}-{}.wav", std::process::id()))
}

fn timing_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.timing.json", path.display()))
}

fn write_wav(path: &Path, start_time: u32, event_onsets: &[usize], sample_rate: u32) {
    let marker = timestamp::encode_for_test(start_time, sample_rate);
    let audio_length = sample_rate as usize * 9;
    let mut samples = vec![0.0f32; marker.len() + audio_length];
    samples[..marker.len()].copy_from_slice(&marker);
    for &onset in event_onsets {
        let onset = marker.len() + onset * sample_rate as usize / 16_000;
        let duration = sample_rate as usize / 3;
        for index in 0..duration {
            let position = onset + index;
            if position >= samples.len() {
                break;
            }
            samples[position] += 0.65
                * (-(index as f32) / (sample_rate as f32 * 0.045)).exp()
                * (2.0 * std::f32::consts::PI * 420.0 * index as f32 / sample_rate as f32).sin();
        }
    }
    let spec = WavSpec { channels: 1, sample_rate, bits_per_sample: 16, sample_format: SampleFormat::Int };
    let mut writer = WavWriter::create(path, spec).unwrap();
    for sample in samples {
        writer.write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).unwrap();
    }
    writer.finalize().unwrap();
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn write_sidecar(path: &Path, sample_rate: u32, clock_start_ns: i64, server: &str) {
    write_sidecar_ex(path, sample_rate, clock_start_ns, server, 1.0, None, false, None);
}

fn write_sidecar_ex(
    path: &Path, sample_rate: u32, clock_start_ns: i64, server: &str,
    max_abs_offset_ms: f64, checked_at: Option<i64>, clock_jump: bool, hash_override: Option<&str>,
) {
    let wav_samples = sample_rate as u64 * 9;
    let nanoseconds_per_sample = 1_000_000_000i64 / sample_rate as i64;
    let bytes = fs::read(path).unwrap();
    let hash = hash_override.map(|s| s.to_string()).unwrap_or_else(|| sha256_hex(&bytes));
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let prefix = timestamp::marker_samples(sample_rate);
    let checked = checked_at.unwrap_or(clock_start_ns - 1_000_000_000);
    let sidecar = json!({
        "schema_version": 2,
        "clock_domain": "windows-utc-synchronized-by-ntp",
        "source": "wasapi-loopback",
        "wav_file": name,
        "wav_sha256": hash,
        "sample_rate": sample_rate,
        "actual_device_sample_rate": sample_rate,
        "first_pcm_utc_unix_ns": clock_start_ns,
        "first_pcm_millis_of_day": 36_000_000,
        "fsk_semantics": "first_pcm_sample",
        "fsk_prefix_samples": prefix,
        "recording_started_unix_ns": clock_start_ns - 500_000_000,
        "recording_ended_unix_ns": clock_start_ns + 9_000_000_000i64,
        "qpc_utc_calibrations": [
            {"phase": "start", "qpc_100ns": 1000, "utc_unix_ns": clock_start_ns - 1_000_000, "span_qpc_100ns": 1},
            {"phase": "end", "qpc_100ns": 1000 + wav_samples * 100, "utc_unix_ns": clock_start_ns + (wav_samples as i64) * nanoseconds_per_sample, "span_qpc_100ns": 1}
        ],
        "clock_jump_detected": clock_jump,
        "anchors": [
            {"wav_sample_index": 0, "device_position": 1000, "qpc_100ns": 2000, "utc_unix_ns": clock_start_ns},
            {"wav_sample_index": wav_samples - 1, "device_position": 1000 + wav_samples - 1, "qpc_100ns": 2000 + (wav_samples - 1) * 100, "utc_unix_ns": clock_start_ns + (wav_samples - 1) as i64 * nanoseconds_per_sample}
        ],
        "discontinuities": [],
        "time_sync": {
            "schema_version": 2,
            "report_kind": "pre_sync",
            "server": server,
            "checked_at_unix_ns": checked,
            "status": "pass",
            "max_abs_offset_ms": max_abs_offset_ms
        }
    });
    fs::write(timing_path(path), serde_json::to_vec_pretty(&sidecar).unwrap()).unwrap();
}

fn clean(paths: &[&Path]) {
    for path in paths {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(timing_path(path));
    }
}

#[test]
fn end_to_end_median_latency() {
    let sender = temporary_path("sender-success");
    let receiver = temporary_path("receiver-success");
    write_wav(&sender, 36_000_000, &[16_000, 48_000, 80_000], 16_000);
    write_wav(&receiver, 36_000_000, &[16_000 + 3_952, 48_000 + 3_952, 80_000 + 3_952], 16_000);
    write_sidecar(&sender, 16_000, 1_700_000_000_000_000_000, "offline-host");
    write_sidecar(&receiver, 16_000, 1_700_000_000_000_000_000, "offline-host");
    let report = analyze_paths(&sender, &receiver, AnalysisOptions { expected_count: Some(3), ..AnalysisOptions::default() });
    assert_eq!(report.status, "success", "{report:?}");
    assert_eq!(report.timing_mode, "sidecar-anchors-v2");
    assert!((report.result.unwrap().latency_ms - 247.0).abs() < 5.0);
    clean(&[&sender, &receiver]);
}

#[test]
fn missing_sidecar_rejected() {
    let sender = temporary_path("sender-no-sidecar");
    let receiver = temporary_path("receiver-no-sidecar");
    write_wav(&sender, 36_000_000, &[16_000], 16_000);
    write_wav(&receiver, 36_000_000, &[16_000 + 3_952], 16_000);
    let report = analyze_paths(&sender, &receiver, AnalysisOptions::default());
    assert_eq!(report.error.unwrap().code, "UNSUPPORTED_TIMING_PROTOCOL");
    clean(&[&sender, &receiver]);
}

#[test]
fn ntp_server_mismatch_rejected() {
    let sender = temporary_path("sender-server-mismatch");
    let receiver = temporary_path("receiver-server-mismatch");
    write_wav(&sender, 36_000_000, &[16_000], 16_000);
    write_wav(&receiver, 36_000_000, &[16_000 + 3_952], 16_000);
    write_sidecar(&sender, 16_000, 1_700_000_000_000_000_000, "host-a");
    write_sidecar(&receiver, 16_000, 1_700_000_000_000_000_000, "host-b");
    let report = analyze_paths(&sender, &receiver, AnalysisOptions::default());
    assert_eq!(report.error.unwrap().code, "UNSUPPORTED_TIMING_PROTOCOL");
    clean(&[&sender, &receiver]);
}

#[test]
fn hash_mismatch_rejected() {
    let sender = temporary_path("sender-hash");
    let receiver = temporary_path("receiver-hash");
    write_wav(&sender, 36_000_000, &[16_000], 16_000);
    write_wav(&receiver, 36_000_000, &[16_000 + 3_952], 16_000);
    write_sidecar_ex(&sender, 16_000, 1_700_000_000_000_000_000, "offline-host", 1.0, None, false, Some("deadbeef"));
    write_sidecar(&receiver, 16_000, 1_700_000_000_000_000_000, "offline-host");
    let report = analyze_paths(&sender, &receiver, AnalysisOptions::default());
    assert_eq!(report.error.unwrap().code, "UNSUPPORTED_TIMING_PROTOCOL");
    clean(&[&sender, &receiver]);
}

#[test]
fn expired_sync_report_rejected() {
    let sender = temporary_path("sender-expired");
    let receiver = temporary_path("receiver-expired");
    write_wav(&sender, 36_000_000, &[16_000], 16_000);
    write_wav(&receiver, 36_000_000, &[16_000 + 3_952], 16_000);
    let start = 1_700_000_000_000_000_000i64;
    write_sidecar_ex(&sender, 16_000, start, "offline-host", 1.0, Some(start - 700_i64 * 1_000_000_000), false, None);
    write_sidecar(&receiver, 16_000, start, "offline-host");
    let report = analyze_paths(&sender, &receiver, AnalysisOptions::default());
    assert_eq!(report.error.unwrap().code, "UNSUPPORTED_TIMING_PROTOCOL");
    clean(&[&sender, &receiver]);
}

#[test]
fn relative_clock_error_exceeded() {
    let sender = temporary_path("sender-clock");
    let receiver = temporary_path("receiver-clock");
    write_wav(&sender, 36_000_000, &[16_000], 16_000);
    write_wav(&receiver, 36_000_000, &[16_000 + 3_952], 16_000);
    write_sidecar_ex(&sender, 16_000, 1_700_000_000_000_000_000, "offline-host", 6.0, None, false, None);
    write_sidecar_ex(&receiver, 16_000, 1_700_000_000_000_000_000, "offline-host", 6.0, None, false, None);
    let report = analyze_paths(&sender, &receiver, AnalysisOptions { max_clock_error_ms: 10.0, ..AnalysisOptions::default() });
    assert_eq!(report.error.unwrap().code, "UNSUPPORTED_TIMING_PROTOCOL");
    clean(&[&sender, &receiver]);
}

#[test]
fn clock_jump_rejected() {
    let sender = temporary_path("sender-jump");
    let receiver = temporary_path("receiver-jump");
    write_wav(&sender, 36_000_000, &[16_000], 16_000);
    write_wav(&receiver, 36_000_000, &[16_000 + 3_952], 16_000);
    write_sidecar_ex(&sender, 16_000, 1_700_000_000_000_000_000, "offline-host", 1.0, None, true, None);
    write_sidecar(&receiver, 16_000, 1_700_000_000_000_000_000, "offline-host");
    let report = analyze_paths(&sender, &receiver, AnalysisOptions::default());
    assert_eq!(report.error.unwrap().code, "UNSUPPORTED_TIMING_PROTOCOL");
    clean(&[&sender, &receiver]);
}

#[test]
fn cross_utc_day_rejected() {
    let sender = temporary_path("sender-day");
    let receiver = temporary_path("receiver-day");
    write_wav(&sender, 36_000_000, &[16_000], 16_000);
    write_wav(&receiver, 36_000_000, &[16_000 + 3_952], 16_000);
    write_sidecar(&sender, 16_000, 1_700_000_000_000_000_000, "offline-host");
    write_sidecar(&receiver, 16_000, 1_700_000_000_000_000_000 + 86_400_i64 * 1_000_000_000, "offline-host");
    let report = analyze_paths(&sender, &receiver, AnalysisOptions::default());
    assert_eq!(report.error.unwrap().code, "UNSUPPORTED_TIMING_PROTOCOL");
    clean(&[&sender, &receiver]);
}

#[test]
fn sample_rate_48k_ok() {
    let sender = temporary_path("sender-48k");
    let receiver = temporary_path("receiver-48k");
    write_wav(&sender, 36_000_000, &[16_000, 48_000], 48_000);
    write_wav(&receiver, 36_000_000, &[16_000 + 3_952, 48_000 + 3_952], 48_000);
    write_sidecar(&sender, 48_000, 1_700_000_000_000_000_000, "offline-host");
    write_sidecar(&receiver, 48_000, 1_700_000_000_000_000_000, "offline-host");
    let report = analyze_paths(&sender, &receiver, AnalysisOptions { expected_count: Some(2), ..AnalysisOptions::default() });
    assert_eq!(report.status, "success", "{report:?}");
    assert!((report.result.unwrap().latency_ms - 247.0).abs() < 5.0);
    clean(&[&sender, &receiver]);
}
