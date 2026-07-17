use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidArgument,
    WavReadFailed,
    UnsupportedAudioFormat,
    TimestampNotFound,
    EventCountMismatch,
    EventDetectionUncertain,
    LatencyOutOfRange,
    LatencyUnstable,
    OutputWriteFailed,
    AnalysisFailed,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::WavReadFailed => "WAV_READ_FAILED",
            Self::UnsupportedAudioFormat => "UNSUPPORTED_AUDIO_FORMAT",
            Self::TimestampNotFound => "TIMESTAMP_NOT_FOUND",
            Self::EventCountMismatch => "EVENT_COUNT_MISMATCH",
            Self::EventDetectionUncertain => "EVENT_DETECTION_UNCERTAIN",
            Self::LatencyOutOfRange => "LATENCY_OUT_OF_RANGE",
            Self::LatencyUnstable => "LATENCY_UNSTABLE",
            Self::OutputWriteFailed => "OUTPUT_WRITE_FAILED",
            Self::AnalysisFailed => "ANALYSIS_FAILED",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckerError {
    pub code: ErrorCode,
    pub message: String,
}

impl CheckerError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for CheckerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for CheckerError {}
