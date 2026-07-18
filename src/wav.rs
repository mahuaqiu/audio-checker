use std::path::Path;

#[derive(Debug, Clone)]
pub struct AudioFile {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

pub fn read_wav(path: &Path) -> Result<AudioFile, String> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| format!("无法读取 WAV 文件 {}: {e}", path.display()))?;
    let spec = reader.spec();

    if spec.channels == 0 {
        return Err("WAV 文件声道数无效".to_string());
    }
    if spec.sample_rate != 16_000 && spec.sample_rate != 48_000 {
        return Err(format!(
            "不支持的采样率 {} Hz，仅支持 16000 Hz 和 48000 Hz",
            spec.sample_rate
        ));
    }
    if spec.bits_per_sample == 0 {
        return Err("WAV 文件位深无效".to_string());
    }

    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => {
            if spec.bits_per_sample != 32 {
                return Err("仅支持 32 位浮点 WAV".to_string());
            }
            reader
                .samples::<f32>()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("读取浮点 WAV 数据失败: {e}"))?
        }
        hound::SampleFormat::Int => match spec.bits_per_sample {
            8 => reader
                .samples::<i8>()
                .map(|s| s.map(|v| v as f32 / 128.0))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("读取 8 位 WAV 数据失败: {e}"))?,
            16 => reader
                .samples::<i16>()
                .map(|s| s.map(|v| v as f32 / 32768.0))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("读取 16 位 WAV 数据失败: {e}"))?,
            24 | 32 => {
                let scale = 2f32.powi(spec.bits_per_sample as i32 - 1);
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|v| v as f32 / scale))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| format!("读取 {} 位 WAV 数据失败: {e}", spec.bits_per_sample))?
            }
            bits => return Err(format!("不支持的整数 WAV 位深: {bits}")),
        },
    };

    let channels = spec.channels as usize;
    if !interleaved.len().is_multiple_of(channels) {
        return Err("WAV 音频数据不是完整的多声道帧".to_string());
    }
    let samples = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect();

    Ok(AudioFile {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        samples,
    })
}

/// 将支持的输入统一到 16 kHz。
pub fn to_analysis_rate(audio: &AudioFile) -> Vec<f32> {
    crate::preprocess::resample_to_16k(&audio.samples, audio.sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 多声道会转换为单声道() {
        let audio = AudioFile {
            sample_rate: 16_000,
            channels: 1,
            samples: vec![0.2, 0.6],
        };
        assert_eq!(to_analysis_rate(&audio), vec![0.2, 0.6]);
    }
}
