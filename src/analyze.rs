use crate::align;
use crate::detector::{self, Event};
use crate::latency;
use crate::timing::{self, ValidatedTiming};
use crate::wav;
use serde::Serialize;
use std::path::{Path, PathBuf};

const ANALYSIS_RATE: f64 = 16_000.0;

#[derive(Debug, Clone)]
pub struct AnalysisOptions {
    pub expected_count: Option<usize>,
    pub min_gap_ms: f64,
    pub max_latency_ms: f64,
    pub max_clock_error_ms: f64,
    pub sender_timing_path: Option<PathBuf>,
    pub receiver_timing_path: Option<PathBuf>,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            expected_count: None,
            min_gap_ms: 2_000.0,
            max_latency_ms: 500.0,
            max_clock_error_ms: 10.0,
            sender_timing_path: None,
            receiver_timing_path: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AnalysisReport {
    pub status: String,
    pub sender_file: String,
    pub receiver_file: String,
    pub timing_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clock_quality: Option<ClockQuality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_start_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receiver_start_time: Option<String>,
    pub event_count: usize,
    pub events: Vec<ReportEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ReportResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ReportError>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ClockQuality {
    pub sender_server: Option<String>,
    pub receiver_server: Option<String>,
    pub sender_offset_ms: Option<f64>,
    pub receiver_offset_ms: Option<f64>,
    pub relative_clock_error_bound_ms: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct ReportEvent {
    pub index: usize,
    pub sender_event_time: Option<String>,
    pub receiver_event_time: Option<String>,
    pub latency_ms: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct ReportResult {
    pub latency_ms: f64,
    pub average_ms: f64,
    pub minimum_ms: f64,
    pub maximum_ms: f64,
}

#[derive(Debug, Serialize)]
pub struct ReportError {
    pub code: String,
    pub message: String,
}

struct PreparedAudio {
    timing: ValidatedTiming,
    analysis_samples: Vec<f32>,
    events: Vec<Event>,
    sample_rate: u32,
}

#[derive(Debug)]
struct AnalysisFailure {
    code: &'static str,
    message: String,
}

impl AnalysisFailure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

pub fn analyze_paths(
    sender_path: &Path,
    receiver_path: &Path,
    options: AnalysisOptions,
) -> AnalysisReport {
    let mut report = AnalysisReport {
        status: "error".to_string(),
        sender_file: sender_path.display().to_string(),
        receiver_file: receiver_path.display().to_string(),
        timing_mode: "sidecar-anchors-v1".to_string(),
        clock_quality: None,
        sender_start_time: None,
        receiver_start_time: None,
        event_count: 0,
        events: Vec::new(),
        result: None,
        error: None,
        warnings: Vec::new(),
    };
    let sender = match prepare(sender_path, options.sender_timing_path.as_deref(), &options) {
        Ok(audio) => audio,
        Err(error) => return set_error(report, error),
    };
    let receiver = match prepare(receiver_path, options.receiver_timing_path.as_deref(), &options) {
        Ok(audio) => audio,
        Err(error) => {
            report.event_count = sender.events.len();
            report.sender_start_time = Some(timing::utc_string(sender.timing.sidecar.first_pcm_utc_unix_ns));
            report.events = sender
                .events
                .iter()
                .enumerate()
                .map(|(index, event)| ReportEvent {
                    index: index + 1,
                    sender_event_time: event_time(&sender, event.onset_sample).ok().map(timing::utc_string),
                    receiver_event_time: None,
                    latency_ms: None,
                })
                .collect();
            return set_error(report, error);
        }
    };
    report.sender_start_time = Some(timing::utc_string(sender.timing.sidecar.first_pcm_utc_unix_ns));
    report.receiver_start_time = Some(timing::utc_string(receiver.timing.sidecar.first_pcm_utc_unix_ns));
    report.warnings.extend(sender.timing.warnings.clone());
    report.warnings.extend(receiver.timing.warnings.clone());

    let sender_sync = sender.timing.sidecar.time_sync.as_ref().unwrap();
    let receiver_sync = receiver.timing.sidecar.time_sync.as_ref().unwrap();
    if sender_sync.server != receiver_sync.server {
        return set_error(report, AnalysisFailure::new(
            "UNSUPPORTED_TIMING_PROTOCOL",
            "发送端和接收端的 NTP server 不一致",
        ));
    }
    let relative_bound = match (sender_sync.max_abs_offset_ms, receiver_sync.max_abs_offset_ms) {
        (Some(left), Some(right)) => Some(left.abs() + right.abs()),
        _ => None,
    };
    report.clock_quality = Some(ClockQuality {
        sender_server: sender_sync.server.clone(),
        receiver_server: receiver_sync.server.clone(),
        sender_offset_ms: sender_sync.max_abs_offset_ms,
        receiver_offset_ms: receiver_sync.max_abs_offset_ms,
        relative_clock_error_bound_ms: relative_bound,
    });
    if relative_bound.is_some_and(|value| value > options.max_clock_error_ms) {
        return set_error(report, AnalysisFailure::new(
            "UNSUPPORTED_TIMING_PROTOCOL",
            format!("两端相对时钟误差上界超过 {:.3}ms", options.max_clock_error_ms),
        ));
    }

    if sender.events.len() != receiver.events.len() {
        report.event_count = sender.events.len().min(receiver.events.len());
        report.events = build_report_events(&sender, &receiver, &sender.events, &receiver.events, None);
        return set_error(report, AnalysisFailure::new(
            "EVENT_COUNT_MISMATCH",
            format!("拨弦事件数量不一致：发送方 {} 次，接收方 {} 次", sender.events.len(), receiver.events.len()),
        ));
    }
    let aligned = match align::refine_events(
        &sender.analysis_samples,
        &receiver.analysis_samples,
        &sender.events,
        &receiver.events,
    ) {
        Ok(aligned) => aligned,
        Err(message) => {
            report.event_count = sender.events.len();
            report.events = build_report_events(&sender, &receiver, &sender.events, &receiver.events, None);
            return set_error(report, AnalysisFailure::new("EVENT_DETECTION_UNCERTAIN", message));
        }
    };
    let latencies: Vec<f64> = aligned
        .iter()
        .map(|(sender_onset, receiver_onset)| {
            let sender_time = event_time(&sender, *sender_onset).unwrap_or(0) as f64;
            let receiver_time = event_time(&receiver, *receiver_onset).unwrap_or(0) as f64;
            (receiver_time - sender_time) / 1_000_000.0
        })
        .collect();
    report.event_count = latencies.len();
    let sender_events: Vec<Event> = aligned.iter().map(|(onset, _)| Event { onset_sample: *onset, end_sample: *onset + 1 }).collect();
    let receiver_events: Vec<Event> = aligned.iter().map(|(_, onset)| Event { onset_sample: *onset, end_sample: *onset + 1 }).collect();
    report.events = build_report_events(&sender, &receiver, &sender_events, &receiver_events, Some(&latencies));
    match latency::summarize(&latencies, options.max_latency_ms) {
        Ok(summary) => {
            report.result = Some(ReportResult {
                latency_ms: round_ms(summary.median_ms),
                average_ms: round_ms(summary.average_ms),
                minimum_ms: round_ms(summary.minimum_ms),
                maximum_ms: round_ms(summary.maximum_ms),
            });
            report.status = "success".to_string();
        }
        Err((code, message)) => report.error = Some(ReportError { code, message }),
    }
    report
}

fn set_error(mut report: AnalysisReport, error: AnalysisFailure) -> AnalysisReport {
    report.error = Some(ReportError { code: error.code.to_string(), message: error.message });
    report
}

fn prepare(path: &Path, timing_path: Option<&Path>, options: &AnalysisOptions) -> Result<PreparedAudio, AnalysisFailure> {
    let audio = wav::read_wav(path).map_err(|message| AnalysisFailure::new("WAV_READ_FAILED", message))?;
    let default_timing_path = timing::default_path(path);
    let timing_path = timing_path.unwrap_or(&default_timing_path);
    let timing = timing::load_and_validate(&timing_path, audio.sample_rate, audio.samples.len())
        .map_err(|message| AnalysisFailure::new("UNSUPPORTED_TIMING_PROTOCOL", message))?;
    let marker_end = timing.sidecar.fsk_prefix_samples;
    if marker_end >= audio.samples.len() {
        return Err(AnalysisFailure::new("UNSUPPORTED_TIMING_PROTOCOL", "FSK 前缀后没有真实 PCM"));
    }
    let analysis_samples = crate::preprocess::remove_dc(&wav::to_analysis_rate(&wav::AudioFile {
        sample_rate: audio.sample_rate,
        channels: audio.channels,
        samples: audio.samples[marker_end..].to_vec(),
    }));
    let events = detector::detect(&analysis_samples, options.expected_count, options.min_gap_ms)
        .map_err(|message| AnalysisFailure::new("EVENT_DETECTION_UNCERTAIN", message))?;
    Ok(PreparedAudio { timing, analysis_samples, events, sample_rate: audio.sample_rate })
}

fn event_time(audio: &PreparedAudio, onset_sample: usize) -> Result<i64, String> {
    let wav_sample = (onset_sample as f64 * audio.sample_rate as f64 / ANALYSIS_RATE).round() as u64;
    audio.timing.event_utc_ns(wav_sample)
}

fn build_report_events(
    sender: &PreparedAudio,
    receiver: &PreparedAudio,
    sender_events: &[Event],
    receiver_events: &[Event],
    latencies: Option<&[f64]>,
) -> Vec<ReportEvent> {
    let count = sender_events.len().max(receiver_events.len());
    (0..count).map(|index| ReportEvent {
        index: index + 1,
        sender_event_time: sender_events.get(index).and_then(|event| event_time(sender, event.onset_sample).ok()).map(timing::utc_string),
        receiver_event_time: receiver_events.get(index).and_then(|event| event_time(receiver, event.onset_sample).ok()).map(timing::utc_string),
        latency_ms: latencies.and_then(|values| values.get(index).copied().map(round_ms)),
    }).collect()
}

fn round_ms(value: f64) -> f64 { (value * 1000.0).round() / 1000.0 }
