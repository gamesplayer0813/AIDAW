# AIDAW

一个面向 Windows 11 的 **AI 驱动 DAW**：AI 通过 [MCP](https://modelcontextprotocol.io) 工具读写工程、作曲、渲染；人在 GUI 里实时看到 AI 的每一步修改，并可以精调、撤销。

![AIDAW GUI](docs/screenshot.png)

## 架构

```
            ┌──────────────┐  stdio JSON-RPC (MCP)  ┌───────────────┐
 Claude 等 ─┤  aidaw-mcp   ├──────── 读/写 ────────►│ song.aidaw.json│
  AI 客户端  └──────────────┘                        └───────┬───────┘
                                                    文件监听 │ 自动保存
                                                  ┌─────────▼────────┐
                                    人 ◄────────► │  aidaw (GUI)      │
                                                  │  egui + cpal 播放 │
                                                  └──────────────────┘
```

| crate | 作用 |
|---|---|
| `aidaw-core` | 工程数据模型（JSON）、编辑操作 `Op`、内置合成器/鼓机、音序器、离线渲染 WAV |
| `aidaw-engine` | 实时播放：cpal → Windows 上走 WASAPI，音频回调里跑同一个音序器 |
| `aidaw-gui` | 桌面编辑器（`aidaw.exe`）：编排时间线、钢琴卷帘、轨道/混音、乐器参数、撤销重做 |
| `aidaw-mcp` | MCP 服务器（`aidaw-mcp.exe`）：让 AI 新建/打开工程、加轨、写音符、批量编辑、渲染 |

**协作模型**：工程就是一个 `.aidaw.json` 文件。MCP 每次调用都“读 → 改 → 原子写回”；GUI 轮询文件，发现外部修改就热重载，并把重载前的版本压进撤销栈 —— 所以 AI 的任何改动在 GUI 里按 `Ctrl+Z` 就能撤回。GUI 里的修改 300ms 后自动保存，AI 下一次调用就能读到。

时间单位一律是**拍**（四分音符）：clip 的 `start` 是绝对拍位置，note 的 `start` 是相对 clip 的拍位置。音高可以写 MIDI 数字或音名（`"C4"` = 60）。

## 构建（Windows 11）

1. 安装 Rust：<https://rustup.rs>（默认 MSVC 工具链，需要 VS Build Tools 的 “C++ 生成工具”）
2. ```powershell
   cargo build --release
   ```
3. 产物：`target\release\aidaw.exe`、`target\release\aidaw-mcp.exe`

CI（GitHub Actions, `windows-latest`）每次 PR 都会构建并上传这两个 exe 作为 artifact。

## 使用

### GUI

第一次打开会出现欢迎页：可以选 **2 分钟互动教程**（一步步教你播放、画音符、改音色，做对了自动进入下一步）、播放 demo、新建空白歌曲，或者按 3 步把 Claude 接进来。之后随时可以在 **❓ Help** 菜单里重新打开。

新手友好设计：乐器用预设选（Soft Keys / Bass / Warm Pad / Drum Kit …，高级参数折叠起来），点钢琴键能直接试听，画音符时能听到声音；**Key + Stay in key** 会高亮调内音并把音符自动吸附过去，乱点也好听；每个按钮都有悬停说明。

```powershell
aidaw.exe examples\demo.aidaw.json
```

- 空格：播放/暂停；编排区点标尺定位播放头
- 编排区：双击空白处新建 clip，拖动移动，拖右边缘改长度，`Del` 删除
- 钢琴卷帘：点击/拖拽空白处画音符，拖动音符改时间和音高，拖右边缘改时值，右键或 `Del` 删除，`Ctrl+滚轮` 缩放
- `Ctrl+Z / Ctrl+Y` 撤销重做，`Ctrl+S` 保存，File → Export WAV 导出

### 让 AI 作曲（Claude Desktop 示例）

`%APPDATA%\Claude\claude_desktop_config.json`：

```json
{
  "mcpServers": {
    "aidaw": {
      "command": "C:\\path\\to\\aidaw-mcp.exe",
      "args": ["--project", "C:\\Music\\song.aidaw.json"]
    }
  }
}
```

然后在 GUI 里打开同一个文件，让 AI 说：“写一段 100 BPM 的 lo-fi，Am-F-C-G，加鼓和贝斯”，它写的东西会实时出现在 GUI 里。

MCP 工具：`new_project`、`open_project`、`get_project`、`apply_ops`（原子批量）、`add_track`、`update_track`、`remove_track`、`add_clip`、`update_clip`、`remove_clip`、`add_notes`、`remove_notes`、`transpose_clip`、`update_project`、`render_wav`。

`examples/demo.aidaw.json` 就是通过 MCP 接口生成的 4 轨 demo。

## 路线图

- [ ] 插件宿主：VST3 / CLAP 乐器与效果器
- [ ] 音频轨与录音、采样器
- [ ] 自动化包络、效果器链（EQ / 压缩 / 混响）
- [ ] ASIO 低延迟驱动、导出 MIDI
- [ ] 循环播放区、多选/复制粘贴、量化
- [ ] 音频回调零分配、引擎与 UI 之间改为无锁命令队列
- [ ] AI 侧：`render_wav` 后回传音频分析（响度/频谱），让 AI 能“听”自己的作品
