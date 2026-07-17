# 会议音频采播时延计算工具实现计划

## 1. 项目目标

开发一个纯 Rust 命令行工具 `audio-checker.exe`，读取发送方和接收方使用 `audio-recorder` 生成的两段 WAV 录音，识别其中一次或多次拨弦事件，并根据事件的绝对时间计算会议系统的音频传输时延。

工具不提供 GUI，要求 Windows EXE 体积尽量小。分析结果同时输出到命令行标准输出，并保存为发送方音频同目录下的 JSON 文件。

## 2. 使用前提

- 两段录音均由 `audio-recorder` 开启 `--timestamp-mark` 生成。
- 两段录音发生在同一天，不处理跨天情况。
- 两台电脑测试前必须完成时钟同步。
- 发送方和接收方包含相同次数的拨弦事件。
- 相邻两次拨弦默认至少间隔 2000 毫秒。
- 单次合理时延范围为 0 至 500 毫秒。
- 拨弦音频由强到弱衰减，单次长度不超过 500 毫秒。
- 优先支持 16000 Hz WAV，兼容 48000 Hz WAV。

## 3. 命令行设计

```powershell
audio-checker.exe --sender sender.wav --receiver receiver.wav --count 3
```

参数：

```text
--sender <PATH>       发送方 WAV，必填
--receiver <PATH>     接收方 WAV，必填
-n, --count <N>       预期拨弦次数，可选但推荐指定
-o, --output <PATH>   JSON 输出路径，可选
--min-gap <MS>        两次事件最小间隔，默认 2000
--max-latency <MS>    最大合理时延，默认 500
--pretty              格式化输出 JSON
--verbose             将分析日志输出到 stderr
-h, --help            显示帮助
-V, --version         显示版本
```

未指定输出路径时，默认在发送方 WAV 同目录生成：

```text
sender.audio-delay.json
```

标准输出只写 JSON，运行日志和警告写入标准错误，方便其他程序直接解析调用结果。

## 4. 分析流程

### 4.1 读取和校验音频

1. 打开发送方和接收方 WAV。
2. 校验文件格式、声道、采样率和数据完整性。
3. 支持 16000 Hz 和 48000 Hz。
4. 多声道输入转换为单声道。
5. 48000 Hz 输入使用带抗混叠滤波的重采样方法转换到 16000 Hz。
6. 不支持的格式直接返回结构化错误，不输出不可靠结果。

### 4.2 解码录音开始时间

兼容 `audio-recorder` 当前的 FSK 标记协议，从 WAV 开头解码当天毫秒数。

当前 FSK 前缀约为 900 毫秒，包括 FSK 数据和保护静音。拨弦事件在实际录音区中的时间需要扣除该前缀：

```text
事件绝对时间 = FSK 录音开始时间 + 事件在实际录音区中的偏移
```

两端任意一个 FSK 标记解码失败时，停止计算并报错。

### 4.3 检测拨弦事件

先在发送端和接收端分别检测拨弦候选：

1. 去除直流偏移并估计背景噪声。
2. 以短窗口计算能量上升、频谱变化和高频瞬态强度。
3. 检测快速起音并在随后 500 毫秒内逐渐衰减的事件。
4. 将 2000 毫秒内的重复峰值合并为同一次拨弦。
5. 使用自适应噪声阈值，避免固定音量阈值导致弱音漏检或噪声误检。

如果指定 `--count N`，两端都必须检测到恰好 N 次事件。未指定时，两端自动检测到的次数必须相同，否则返回 `EVENT_COUNT_MISMATCH`。

### 4.4 高精度事件定位

每次事件分三步定位：

1. 使用短时能量和频谱变化找到粗略起音位置。
2. 提取发送端拨弦的多频带能量包络和起音形状，在接收端对应候选附近进行特征相关搜索。
3. 在最佳位置附近使用原始 PCM 包络、起音斜率和局部相关峰值细化事件时间。

事件时间不使用最大音量点。最大音量通常晚于真实起音，会议系统的自动增益也会改变最大音量位置。统一使用拨弦起音累计强度达到设定比例的位置作为时间基准，并通过局部相关结果修正。

算法判断不可靠时应报错，不能为了凑够事件数量强行选择低质量候选。

### 4.5 事件配对和时延计算

由于两端拨弦次数确定相同，事件按时间顺序一一配对：

```text
发送方第 1 次 -> 接收方第 1 次
发送方第 2 次 -> 接收方第 2 次
发送方第 N 次 -> 接收方第 N 次
```

每次时延计算公式：

```text
时延 = 接收方事件绝对时间 - 发送方事件绝对时间
```

每一对事件还需要检查：

- 时延不得小于 0 毫秒。
- 时延不得大于 500 毫秒。
- 两端相邻拨弦的时间间隔应基本一致。
- 多次拨弦的时延不应出现无法解释的大幅波动。

任意事件不满足关键条件时，整体状态返回失败，但 JSON 仍保留已经识别出的事件及其时延，方便排查。

最终推荐时延使用所有有效事件时延的中位数，同时输出平均值、最小值和最大值。

## 5. 精简 JSON 输出

对外结果只保留录音开始时间、每次事件时间、每次时延和汇总数据，不输出采样率、内部匹配分数或算法特征。

成功示例：

```json
{
  "status": "success",
  "sender_file": "D:\\records\\sender.wav",
  "receiver_file": "D:\\records\\receiver.wav",
  "sender_start_time": "10:00:00.123",
  "receiver_start_time": "10:00:00.600",
  "event_count": 3,
  "events": [
    {
      "index": 1,
      "sender_event_time": "10:00:04.513",
      "receiver_event_time": "10:00:04.760",
      "latency_ms": 247.0
    },
    {
      "index": 2,
      "sender_event_time": "10:00:08.220",
      "receiver_event_time": "10:00:08.465",
      "latency_ms": 245.0
    },
    {
      "index": 3,
      "sender_event_time": "10:00:12.105",
      "receiver_event_time": "10:00:12.354",
      "latency_ms": 249.0
    }
  ],
  "result": {
    "latency_ms": 247.0,
    "average_ms": 247.0,
    "minimum_ms": 245.0,
    "maximum_ms": 249.0
  },
  "warnings": []
}
```

其中：

- `sender_start_time` 和 `receiver_start_time` 是 FSK 解码出的录音开始时间。
- `sender_event_time` 和 `receiver_event_time` 是每次拨弦的绝对时间。
- `latency_ms` 是这一对拨弦事件的时延。
- `result.latency_ms` 是最终推荐值，采用中位数。

失败示例：

```json
{
  "status": "error",
  "sender_file": "D:\\records\\sender.wav",
  "receiver_file": "D:\\records\\receiver.wav",
  "sender_start_time": "10:00:00.123",
  "receiver_start_time": "10:00:00.600",
  "event_count": 2,
  "events": [
    {
      "index": 1,
      "sender_event_time": "10:00:04.513",
      "receiver_event_time": "10:00:04.760",
      "latency_ms": 247.0
    },
    {
      "index": 2,
      "sender_event_time": "10:00:08.220",
      "receiver_event_time": "10:00:08.830",
      "latency_ms": 610.0
    }
  ],
  "error": {
    "code": "LATENCY_OUT_OF_RANGE",
    "message": "第 2 次拨弦时延为 610 毫秒，超过最大允许值 500 毫秒"
  },
  "warnings": []
}
```

## 6. 工程结构

```text
audio-checker/
├── Cargo.toml
├── src/
│   ├── main.rs          命令入口和流程调度
│   ├── cli.rs           命令行参数
│   ├── wav.rs           WAV 读取和格式转换
│   ├── timestamp.rs     FSK 时间标记解码
│   ├── preprocess.rs    滤波、单声道化和重采样
│   ├── detector.rs      拨弦候选检测
│   ├── align.rs         两端事件高精度定位
│   ├── latency.rs       配对、时延和汇总计算
│   ├── output.rs        JSON 和文件输出
│   └── error.rs         错误码定义
└── tests/
    ├── timestamp_tests.rs
    ├── detector_tests.rs
    ├── latency_tests.rs
    └── fixtures/
```

## 7. 依赖和体积控制

优先使用轻量依赖：

```toml
hound = "3.5"
lexopt = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

滤波、短时特征、FSK 解码、局部相关和统计计算直接使用 Rust 实现，避免引入 FFmpeg、机器学习运行时或大型 DSP 框架。

Release 配置：

```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
panic = "abort"
```

## 8. 错误处理

主要错误码：

| 错误码 | 含义 |
|---|---|
| `INVALID_ARGUMENT` | 命令行参数错误 |
| `WAV_READ_FAILED` | WAV 无法读取或数据损坏 |
| `UNSUPPORTED_AUDIO_FORMAT` | 音频格式或采样率不支持 |
| `TIMESTAMP_NOT_FOUND` | 一端或两端 FSK 时间标记解码失败 |
| `EVENT_COUNT_MISMATCH` | 检测次数不一致或不等于指定次数 |
| `EVENT_DETECTION_UNCERTAIN` | 某次拨弦无法可靠定位 |
| `LATENCY_OUT_OF_RANGE` | 某次时延不在 0 至 500 毫秒范围内 |
| `LATENCY_UNSTABLE` | 多次计算结果波动过大 |
| `OUTPUT_WRITE_FAILED` | JSON 文件写入失败 |

失败时仍应尽可能输出已获得的开始时间和事件明细。

## 9. 精度保障和测试

测试分为四层：

1. FSK 测试：验证 16000 Hz、48000 Hz 和不同录音开始时间的解码结果。
2. 合成事件测试：生成已知起音位置和已知时延的拨弦信号，检查定位误差。
3. 损伤模拟测试：加入噪声、音量变化、削波、带宽限制、压缩和平滑，验证会议音频受损后的定位稳定性。
4. 真实会议测试：在实际会议系统中录制多组样本，与人工标注或受控基准对比。

验收建议：

- 干净音频事件定位误差不超过 3 毫秒。
- 常见会议编码和降噪场景误差目标不超过 10 毫秒。
- 无法满足精度要求的事件必须报告不确定，不能输出伪精确结果。
- 16000 Hz 和 48000 Hz 对同一测试信号的结果差异不超过 2 毫秒。
- 指定事件次数、事件数不一致、时延超过 500 毫秒等错误路径均有自动测试。

## 10. 实施步骤

### 阶段一：基础能力

- 初始化 Rust CLI 工程和 Release 体积配置。
- 完成参数解析、WAV 读取和统一数据模型。
- 移植并测试 `audio-recorder` 的 FSK 解码逻辑。
- 完成精简 JSON 输出和错误码框架。

### 阶段二：事件检测

- 实现背景噪声估计和短时能量特征。
- 实现频谱变化与拨弦衰减特征。
- 实现 2000 毫秒最小间隔和候选合并。
- 支持自动事件数及 `--count` 严格校验。

### 阶段三：精确对齐

- 提取发送端事件模板。
- 在接收端对应区域进行多频带特征搜索。
- 使用原始 PCM 和局部相关细化事件时间。
- 实现 0 至 500 毫秒时延校验及稳定性检查。

### 阶段四：验证和打包

- 建立合成、损伤模拟和真实会议测试集。
- 根据测试集调整阈值，但不针对单个样本硬编码。
- 完成 Windows Release 构建并检查 EXE 体积。
- 验证 stdout JSON、同目录文件输出和退出码行为。

## 11. 已知精度前提

最终结果依赖两台录音电脑的系统时钟。如果两台电脑存在时间差，该时间差会直接进入测量结果，因此测试前必须同步时钟。

此外，当前 `audio-recorder` 的 FSK 时间是在音频设备正式产生第一帧数据前获取的，可能引入录音启动误差。第一版先兼容现有格式；如果真实测试表明该误差影响验收精度，应同步升级 `audio-recorder`，将时间标记绑定到实际第一帧 PCM 的系统时间。

## 12. 实现状态

本计划已在当前 Rust 工程中完成第一版实现：

- 已完成 CLI 参数解析、帮助/版本输出、默认 JSON 路径和 `--verbose` 日志。
- 已完成 16 kHz/48 kHz WAV 读取、多声道转单声道、格式校验和抗混叠降采样。
- 已完成 FSK 时间标记解码、开头保护静音搜索和当天毫秒时间格式化。
- 已完成直流去除、自适应噪声阈值、短时能量、瞬态特征、起音检测和最小间隔合并。
- 已完成发送方模板的多帧包络相关、起音位置细化、事件顺序配对和时延稳定性检查。
- 已完成成功/失败 JSON、事件明细保留、结构化错误码、文件输出和退出码行为。
- 已加入 FSK、预处理、检测、统计和 16 kHz/48 kHz 端到端合成 WAV 自动测试。

当前仍依赖实施前提中的两台电脑时钟同步和 `audio-recorder` 的现有 FSK 协议。真实会议录音样本需要在目标设备上继续做验收测试；测试结果不能由合成样本替代。
