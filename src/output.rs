use crate::analyze::AnalysisReport;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub fn default_output(sender: &Path) -> PathBuf {
    let stem = sender
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("sender");
    sender.with_file_name(format!("{stem}.audio-delay.json"))
}

pub fn write_report(path: &Path, json: &str) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|value| !value.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, json)
}

pub fn mark_output_error(report: &mut AnalysisReport, path: &Path, error: &io::Error) {
    report.status = "error".to_string();
    report.result = None;
    report.error = Some(crate::analyze::ReportError {
        code: "OUTPUT_WRITE_FAILED".to_string(),
        message: format!("无法写入结果文件 {}: {error}", path.display()),
    });
}
