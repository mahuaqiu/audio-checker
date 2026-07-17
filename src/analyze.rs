use crate::align;
use crate::detector::{self, Event};
use crate::latency;
use crate::timestamp::{self, TimestampMark};
use crate::wav;
use serde::Serialize;
use std::path::Path;

const ANALYSIS_RATE: f64 = 16_000.0;

#[derive(Debug, Clone, Copy)]
pub struct AnalysisOptions {
    pub expected_count: Option<usize>,
    pub min_gap_ms: f64,
    pub max_latency_ms: f64,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            expected_count: None,
            min_gap_ms: 2_000.0,
            max_latency_ms: 500.0,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AnalysisReport {
    pub status: String,
    pub sender_file: String,
    pub receiver_file: String,
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
    timestamp: TimestampMark,
    timestamp_rate: u32,
    marker_offset_samples: usize,
    analysis_samples: Vec<f32>,
    events: Vec<Event>,
}

#[derive(Debug)]
struct AnalysisFailure {
    code: &'static str,
    message: String,
}

impl AnalysisFailure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
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
        sender_start_time: None,
        receiver_start_time: None,
        event_count: 0,
        events: Vec::new(),
        result: None,
        error: None,
        warnings: Vec::new(),
    };

    let sender = match prepare(sender_path, options.expected_count, options.min_gap_ms) {
        Ok(audio) => audio,
        Err(error) => {
            report.error = Some(ReportError {
                code: error.code.to_string(),
                message: error.message,
            });
            return report;
        }
    };
    report.sender_start_time = Some(timestamp::format_time(
        sender.timestamp.millis_of_day as f64,
    ));

    let receiver = match prepare(receiver_path, options.expected_count, options.min_gap_ms) {
        Ok(audio) => audio,
        Err(error) => {
            report.event_count = sender.events.len();
            report.events = sender
                .events
                .iter()
                .enumerate()
                .map(|(index, event)| ReportEvent {
                    index: index + 1,
                    sender_event_time: Some(timestamp::format_time(event_time(
                        sender.timestamp,
                        sender.timestamp_rate,
                        sender.marker_offset_samples,
                        event.onset_sample,
                    ))),
                    receiver_event_time: None,
                    latency_ms: None,
                })
                .collect();
            report.error = Some(ReportError {
                code: error.code.to_string(),
                message: error.message,
            });
            return report;
        }
    };
    report.receiver_start_time = Some(timestamp::format_time(
        receiver.timestamp.millis_of_day as f64,
    ));

    if sender.events.len() != receiver.events.len() {
        report.event_count = sender.events.len().min(receiver.events.len());
        report.events =
            build_report_events(&sender, &receiver, &sender.events, &receiver.events, None);
        report.error = Some(ReportError {
            code: "EVENT_COUNT_MISMATCH".to_string(),
            message: format!(
                "拨弦事件数量不一致：发送方 {} 次，接收方 {} 次",
                sender.events.len(),
                receiver.events.len()
            ),
        });
        return report;
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
            report.error = Some(ReportError {
                code: "EVENT_DETECTION_UNCERTAIN".to_string(),
                message,
            });
            return report;
        }
    };

    let latencies: Vec<f64> = aligned
        .iter()
        .map(|(sender_onset, receiver_onset)| {
            event_time(
                receiver.timestamp,
                receiver.timestamp_rate,
                receiver.marker_offset_samples,
                *receiver_onset,
            ) - event_time(
                sender.timestamp,
                sender.timestamp_rate,
                sender.marker_offset_samples,
                *sender_onset,
            )
        })
        .collect();
    report.event_count = latencies.len();
    report.events = build_report_events(
        &sender,
        &receiver,
        &aligned
            .iter()
            .map(|(onset, _)| Event {
                onset_sample: *onset,
                end_sample: *onset + 1,
            })
            .collect::<Vec<_>>(),
        &aligned
            .iter()
            .map(|(_, onset)| Event {
                onset_sample: *onset,
                end_sample: *onset + 1,
            })
            .collect::<Vec<_>>(),
        Some(&latencies),
    );

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
        Err((code, message)) => {
            report.error = Some(ReportError { code, message });
        }
    }
    report
}

fn prepare(
    path: &Path,
    expected_count: Option<usize>,
    min_gap_ms: f64,
) -> Result<PreparedAudio, AnalysisFailure> {
    let audio = wav::read_wav(path).map_err(|message| {
        if message.contains("无法读取 WAV")
            || message.contains("读取") && message.contains("数据失败")
            || message.contains("完整的多声道帧")
        {
            AnalysisFailure::new("WAV_READ_FAILED", message)
        } else {
            AnalysisFailure::new("UNSUPPORTED_AUDIO_FORMAT", message)
        }
    })?;
    let (timestamp, marker_offset_samples) =
        timestamp::decode_with_offset(&audio.samples, audio.sample_rate).ok_or_else(|| {
            AnalysisFailure::new("TIMESTAMP_NOT_FOUND", "未检测到有效的 FSK 录音开始时间标记")
        })?;
    let analysis_samples = wav::to_analysis_rate(&audio);
    let marker_end = (((marker_offset_samples + timestamp.marker_samples) as f64 * ANALYSIS_RATE
        / audio.sample_rate as f64)
        .round() as usize)
        .min(analysis_samples.len());
    if marker_end >= analysis_samples.len() {
        return Err(AnalysisFailure::new(
            "TIMESTAMP_NOT_FOUND",
            "FSK 时间标记之后没有可分析的录音数据",
        ));
    }

    let actual_audio = crate::preprocess::remove_dc(&analysis_samples[marker_end..]);
    let events =
        detector::detect(&actual_audio, expected_count, min_gap_ms).map_err(|message| {
            let code = if message.contains("数量") {
                "EVENT_COUNT_MISMATCH"
            } else {
                "EVENT_DETECTION_UNCERTAIN"
            };
            AnalysisFailure::new(code, message)
        })?;
    Ok(PreparedAudio {
        timestamp,
        timestamp_rate: audio.sample_rate,
        marker_offset_samples,
        analysis_samples: actual_audio,
        events,
    })
}

fn event_time(
    timestamp: TimestampMark,
    timestamp_rate: u32,
    marker_offset_samples: usize,
    onset_sample: usize,
) -> f64 {
    timestamp.millis_of_day as f64
        + (marker_offset_samples + timestamp.marker_samples) as f64 * 1000.0 / timestamp_rate as f64
        + onset_sample as f64 * 1000.0 / ANALYSIS_RATE
}

fn build_report_events(
    sender: &PreparedAudio,
    receiver: &PreparedAudio,
    sender_events: &[Event],
    receiver_events: &[Event],
    latencies: Option<&[f64]>,
) -> Vec<ReportEvent> {
    let count = sender_events.len().max(receiver_events.len());
    (0..count)
        .map(|index| {
            let sender_event_time = sender_events.get(index).map(|event| {
                timestamp::format_time(event_time(
                    sender.timestamp,
                    sender.timestamp_rate,
                    sender.marker_offset_samples,
                    event.onset_sample,
                ))
            });
            let receiver_event_time = receiver_events.get(index).map(|event| {
                timestamp::format_time(event_time(
                    receiver.timestamp,
                    receiver.timestamp_rate,
                    receiver.marker_offset_samples,
                    event.onset_sample,
                ))
            });
            ReportEvent {
                index: index + 1,
                sender_event_time,
                receiver_event_time,
                latency_ms: latencies.and_then(|values| values.get(index).copied().map(round_ms)),
            }
        })
        .collect()
}

fn round_ms(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 时间格式和事件偏移保持毫秒精度() {
        let mark = TimestampMark {
            millis_of_day: 36_000_000,
            marker_samples: 14_400,
        };
        assert_eq!(event_time(mark, 16_000, 0, 16_000), 36_001_900.0);
    }
}
