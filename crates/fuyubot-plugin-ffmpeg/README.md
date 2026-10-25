# fuyubot-plugin-ffmpeg

转码插件：注册 `media.ffmpeg`。三个能力：**探测**、**选路**、**执行 + 实时进度**。

```
业务插件 ──get_service("media.ffmpeg")──▶ 本插件 ──ffmpeg 子进程──▶ 文件
                                            │
                                            └─host.emit("ffmpeg.progress", {...})
```

## 启动配置

```json
{ "ffmpeg": "D:/runtime/ffmpeg/ffmpeg.exe", "probe_on_start": true }
```

`ffmpeg` 省略时按 `FFMPEG_DIR` → 插件同级 → 同级 `ffmpeg/` → `PATH` 找；`ffprobe` 同样。
`probe_on_start` 只在启动时做「找二进制 + 列编码器」，**不做**硬件实测（那要编帧，启动会变慢）。

## 方法

| 方法 | 入参 | 说明 |
| --- | --- | --- |
| `ping` | – | 版本 + 正在跑的任务数 |
| `capabilities` | `probe_hardware?` | 编码器表；`probe_hardware:true` 会对每个硬件编码器真编一帧 |
| `probe` | `input` | 时长/编码/分辨率/帧率（ffprobe，缺失时解析 `ffmpeg -i`） |
| `plan` | `input, output, intent?, crf?, preset?, keep_audio?, subtitle?, subtitle_stream?` | 只算方案，不执行（调试选路用） |
| `transcode` | 同上 | 起任务，立刻返回 `job_id` |
| `extract_subtitle` | `input, output, stream?` | 把内嵌字幕抽成外挂 `.ass`；没有字幕流返回 `{extracted:false}` |
| `job_status` | `id` | 单任务快照（percent/speed/fps/eta） |
| `job_list` | – | 全部任务 |
| `job_cancel` | `id` | 取消（下一个进度事件到达时生效，通常 <1s） |
| `prune` | `keep?` | 清理已结束任务记录 |

`intent` 四档：

| intent | 含义 | 选路 |
| --- | --- | --- |
| `remux` | 只换容器 | 能 copy 就 copy |
| `compat`（默认） | 什么设备都能播 | h264：硬件优先，软件兜底 |
| `small` | 体积优先 | hevc：硬件优先，`libx265` 兜底 |
| `fast` | 速度优先 | 只用硬件编码器（没有就降级并说明） |

## 「高度优化」具体优化了什么

1. **能不转就不转。** 目标容器装得下原编码（h264/hevc + aac/mp3）时走 `-c copy`：
   实测 2 秒素材 **252ms** 完成、画质零损失。TS 版是「先 mkv→mp4 全部重编码」，这一步省掉的是最多的时间。
2. **硬件编码器要实测，不看名单。** `-encoders` 里有 `h264_nvenc` 不代表能跑（没驱动、没设备照样列出来）。
   每个候选都真编一帧，结果缓存。本机实测：`h264_nvenc` / `hevc_nvenc` / `h264_amf` / `h264_mf` 可用，
   `h264_qsv` / `hevc_qsv` / `*_vaapi` 不可用——只按名单选会一路失败重试。
3. **音频默认直通。** 源是 aac/mp3 就不重编码，只有 flac/ac3 这类才转 aac 160k。
4. **进度是推的。** `ffmpeg.progress` 事件带 `percent / speed / fps / frame / eta_ms`，
   按 400ms 节流（ffmpeg 默认 0.5s 一条，不节流就是刷屏）。同时注册表留着，`job_status` 也能轮询。
5. **不卡宿主。** 每个任务一个线程 + 一个子进程，`-nostdin` 防止 ffmpeg 抢 stdin；
   取消是标记位 + kill，不会留下僵尸进程。

## 事件

| 事件 | 时机 | 载荷 |
| --- | --- | --- |
| `ffmpeg.progress` | 转码中，≥400ms 一条 | 见下 |
| `ffmpeg.done` | 成功 | 同上，`percent=1.0` |
| `ffmpeg.failed` | 失败 | 额外 `error` |
| `ffmpeg.canceled` | 被取消 | |

```json
{"id":"ff-18f3a1-3","state":"running","percent":42.7,"speed":8.2,"fps":23.9,
 "frame":1024,"out_time_ms":614000,"duration_ms":1440000,"eta_ms":101000,
 "mode":"encode","encoder":"hevc_nvenc"}
```

## 冒烟测试

```bash
cargo run -p fuyubot-plugin-ffmpeg --example plan_smoke
```

生成 2 秒素材 → 探测硬件 → 给出 `compat`/`small` 两套方案 → **真跑一遍并打印进度**。
本机结果：compat 走 copy（252ms）、small 走 `hevc_nvenc`（569ms），两者都落到 100%。

`subtitle` 给一个外挂字幕路径就会烧进画面（`-vf subtitles=...`），此时**强制重编码**——
`-c copy` 没法过滤镜。选流默认「简体中文优先，否则第一条」，也可以先用
`extract_subtitle` 抽成 `.ass` 再传 `subtitle`；抽不出来或滤镜跑挂，调用方应回退成不烧字幕。

## 已知边界

- 字幕：mp4 输出不保留字幕轨（要保留请输出 mkv，那里是 `-c:s copy`；要显示就烧录）；
- 烧录依赖 ffmpeg 的 `--enable-libass`（字幕滤镜）。缺 libass 时滤镜会直接失败，业务侧要回退；
- 10bit / HDR 没有专门处理，`hevc_nvenc` 会按 8bit 输出，需要 HDR 时要显式指定 `-pix_fmt p010le` 等参数；
- 硬件探测是「能编一帧」，不保证长片稳定（过热降频、显存不足仍可能中途失败）。
