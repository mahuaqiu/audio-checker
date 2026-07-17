use audio_checker::output;
use audio_checker::{analyze_paths, AnalysisOptions, AnalysisReport};
use lexopt::prelude::*;
use std::ffi::OsString;
use std::path::PathBuf;

struct Cli {
    sender: PathBuf,
    receiver: PathBuf,
    output: Option<PathBuf>,
    options: AnalysisOptions,
    pretty: bool,
    verbose: bool,
}

fn main() {
    let cli = match parse_args() {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("错误: {error}");
            print_usage();
            std::process::exit(2);
        }
    };

    let mut report = analyze_paths(&cli.sender, &cli.receiver, cli.options);
    let output_path = cli
        .output
        .unwrap_or_else(|| output::default_output(&cli.sender));
    let json = match if cli.pretty {
        serde_json::to_string_pretty(&report)
    } else {
        serde_json::to_string(&report)
    } {
        Ok(json) => json,
        Err(error) => {
            eprintln!("错误: JSON 序列化失败: {error}");
            std::process::exit(1);
        }
    };

    if let Err(error) = output::write_report(&output_path, &json) {
        if cli.verbose {
            eprintln!("警告: 结果文件写入失败: {error}");
        }
        output::mark_output_error(&mut report, &output_path, &error);
    } else if cli.verbose {
        eprintln!("分析完成，结果已写入 {}", output_path.display());
    }

    let json = match if cli.pretty {
        serde_json::to_string_pretty(&report)
    } else {
        serde_json::to_string(&report)
    } {
        Ok(json) => json,
        Err(error) => {
            eprintln!("错误: JSON 序列化失败: {error}");
            std::process::exit(1);
        }
    };
    println!("{json}");
    if report.status != "success" {
        std::process::exit(1);
    }
}

fn parse_args() -> Result<Cli, String> {
    let mut sender = None;
    let mut receiver = None;
    let mut output_path = None;
    let mut options = AnalysisOptions::default();
    let mut pretty = false;
    let mut verbose = false;
    let mut parser = lexopt::Parser::from_env();

    while let Some(arg) = parser
        .next()
        .map_err(|error| format!("参数解析失败: {error}"))?
    {
        match arg {
            Short('s') | Long("sender") => sender = Some(path_value(&mut parser, "--sender")?),
            Short('r') | Long("receiver") => {
                receiver = Some(path_value(&mut parser, "--receiver")?)
            }
            Short('o') | Long("output") => output_path = Some(path_value(&mut parser, "--output")?),
            Short('n') | Long("count") => {
                let count = number_value(&mut parser, "--count")?;
                if count == 0 {
                    return Err("--count 必须大于 0".to_string());
                }
                options.expected_count = Some(count);
            }
            Long("min-gap") => {
                options.min_gap_ms = float_value(&mut parser, "--min-gap")?;
            }
            Long("max-latency") => {
                options.max_latency_ms = float_value(&mut parser, "--max-latency")?;
            }
            Long("pretty") => pretty = true,
            Long("verbose") => verbose = true,
            Short('h') | Long("help") => {
                print_usage();
                std::process::exit(0);
            }
            Short('V') | Long("version") => {
                println!("audio-checker {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            Value(value) => return Err(format!("未知参数: {}", value.to_string_lossy())),
            _ => return Err(format!("未知参数: {arg:?}")),
        }
    }

    let sender = sender.ok_or("缺少 --sender")?;
    let receiver = receiver.ok_or("缺少 --receiver")?;
    if !options.min_gap_ms.is_finite()
        || options.min_gap_ms <= 0.0
        || !options.max_latency_ms.is_finite()
        || options.max_latency_ms <= 0.0
    {
        return Err("--min-gap 和 --max-latency 必须是大于 0 的有限数字".to_string());
    }
    Ok(Cli {
        sender,
        receiver,
        output: output_path,
        options,
        pretty,
        verbose,
    })
}

fn path_value(parser: &mut lexopt::Parser, name: &str) -> Result<PathBuf, String> {
    let value: OsString = parser
        .value()
        .map_err(|error| format!("{name} 需要参数: {error}"))?;
    Ok(PathBuf::from(value))
}

fn number_value(parser: &mut lexopt::Parser, name: &str) -> Result<usize, String> {
    let value: OsString = parser
        .value()
        .map_err(|error| format!("{name} 需要参数: {error}"))?;
    value
        .to_string_lossy()
        .parse()
        .map_err(|_| format!("{name} 参数无效"))
}

fn float_value(parser: &mut lexopt::Parser, name: &str) -> Result<f64, String> {
    let value: OsString = parser
        .value()
        .map_err(|error| format!("{name} 需要参数: {error}"))?;
    value
        .to_string_lossy()
        .parse()
        .map_err(|_| format!("{name} 参数无效"))
}

fn print_usage() {
    println!("用法: audio-checker.exe --sender <PATH> --receiver <PATH> [选项]");
    println!("  --sender <PATH>       发送方 WAV，必填");
    println!("  --receiver <PATH>     接收方 WAV，必填");
    println!("  -n, --count <N>       预期拨弦次数");
    println!("  -o, --output <PATH>   JSON 输出路径");
    println!("      --min-gap <MS>    事件最小间隔，默认 2000");
    println!("      --max-latency <MS> 最大合理时延，默认 500");
    println!("      --pretty          格式化 JSON");
    println!("      --verbose         输出分析日志到 stderr");
    println!("  -h, --help            显示帮助");
    println!("  -V, --version         显示版本");
}

#[allow(dead_code)]
fn _keep_report_type(_: AnalysisReport) {}
