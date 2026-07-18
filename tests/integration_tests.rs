use audio_checker::{analyze_paths, timestamp, AnalysisOptions};
use hound::{SampleFormat, WavSpec, WavWriter};
use serde_json::json;
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

    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec).unwrap();
    for sample in samples {
        writer
            .write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
}

fn write_sidecar(path: &Path, sample_rate: u32, clock_start_ns: i64, server: &str) {
    let wav_samples = sample_rate as u64 * 9;
    let nanoseconds_per_sample = 1_000_000_000i64 / sample_rate as i64;
    let sidecar = json!({
        "schema_version": 1,
        "clock_domain": "windows-utc-synchronized-by-ntp",
        "source": "wasapi-loopback",
        "wav_file": path.display().to_string(),
        "sample_rate": sample_rate,
        "actual_device_sample_rate": sample_rate,
        "first_pcm_utc_unix_ns": clock_start_ns,
        "first_pcm_millis_of_day": 36_000_000,
        "fsk_semantics": "first_pcm_sample",
        "fsk_prefix_samples": timestamp::marker_samples(sample_rate),
        "anchors": [
            {
                "wav_sample_index": 0,
                "device_position": 1000,
                "qpc_100ns": 2000,
                "utc_unix_ns": clock_start_ns
            },
            {
                "wav_sample_index": wav_samples - 1,
                "device_position": 1000 + wav_samples - 1,
                "qpc_100ns": 2000 + (wav_samples - 1) * 100,
                "utc_unix_ns": clock_start_ns + (wav_samples - 1) as i64 * nanoseconds_per_sample
            }
        ],
        "discontinuities": [],
        "time_sync": {
            "server": server,
            "checked_at_unix_ns": clock_start_ns - 1_000_000,
            "status": "pass",
            "max_abs_offset_ms": 1.0
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
fn 端到端计算三次拨弦的中位时延() {
    let sender = temporary_path("sender-success");
    let receiver = temporary_path("receiver-success");
    let sender_events = [16_000usize, 48_000, 80_000];
    let receiver_events = [16_000usize + 3_952, 48_000 + 3_952, 80_000 + 3_952];
    write_wav(&sender, 36_000_000, &sender_events, 16_000);
    write_wav(&receiver, 36_000_000, &receiver_events, 16_000);
    write_sidecar(&sender, 16_000, 1_700_000_000_000_000_000, "offline-host");
    write_sidecar(&receiver, 16_000, 1_700_000_000_000_000_000, "offline-host");

    let report = analyze_paths(
        &sender,
        &receiver,
        AnalysisOptions { expected_count: Some(3), ..AnalysisOptions::default() },
    );
    assert_eq!(report.status, "success", "{report:?}");
    assert_eq!(report.event_count, 3);
    let result = report.result.unwrap();
    assert!((result.latency_ms - 247.0).abs() < 5.0);
    assert_eq!(report.timing_mode, "sidecar-anchors-v1");
    assert!(report.error.is_none());
    clean(&[&sender, &receiver]);
}

#[test]
fn 时延超过上限仍保留事件明细() {
    let sender = temporary_path("sender-range");
    let receiver = temporary_path("receiver-range");
    write_wav(&sender, 36_000_000, &[16_000], 16_000);
    write_wav(&receiver, 36_000_000, &[16_000 + 9_760], 16_000);
    write_sidecar(&sender, 16_000, 1_700_000_000_000_000_000, "offline-host");
    write_sidecar(&receiver, 16_000, 1_700_000_000_000_000_000, "offline-host");

    let report = analyze_paths(&sender, &receiver, AnalysisOptions { expected_count: Some(1), ..AnalysisOptions::default() });
    assert_eq!(report.status, "error");
    assert_eq!(report.error.unwrap().code, "LATENCY_OUT_OF_RANGE");
    assert_eq!(report.events.len(), 1);
    assert!(report.events[0].latency_ms.unwrap() > 500.0);
    clean(&[&sender, &receiver]);
}

#[test]
fn 指定次数不一致返回事件数量错误() {
    let sender = temporary_path("sender-count");
    let receiver = temporary_path("receiver-count");
    write_wav(&sender, 36_000_000, &[16_000, 48_000], 16_000);
    write_wav(&receiver, 36_000_000, &[16_000], 16_000);
    write_sidecar(&sender, 16_000, 1_700_000_000_000_000_000, "offline-host");
    write_sidecar(&receiver, 16_000, 1_700_000_000_000_000_000, "offline-host");

    let report = analyze_paths(&sender, &receiver, AnalysisOptions::default());
    assert_eq!(report.status, "error");
    assert_eq!(report.error.unwrap().code, "EVENT_COUNT_MISMATCH");
    clean(&[&sender, &receiver]);
}

#[test]
fn 四十八千赫兹输入不会放大时间前缀() {
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

#[test]
fn 缺少_sidecar_时拒绝旧协议() {
    let sender = temporary_path("sender-no-sidecar");
    let receiver = temporary_path("receiver-no-sidecar");
    write_wav(&sender, 36_000_000, &[16_000], 16_000);
    write_wav(&receiver, 36_000_000, &[16_000 + 3_952], 16_000);
    let report = analyze_paths(&sender, &receiver, AnalysisOptions::default());
    assert_eq!(report.error.unwrap().code, "UNSUPPORTED_TIMING_PROTOCOL");
    clean(&[&sender, &receiver]);
}

#[test]
fn 两端_ntp_server_不一致时拒绝计算() {
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
