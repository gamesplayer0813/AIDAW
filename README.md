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

**协作模型**：工程就是一个 `.aidaw.json` 文件。MCP 每次调用都“读 → 改 → 原子写回”，写回前发现文件被 GUI 改过就在新版本上重放这次编辑；GUI 轮询文件，发现外部修改就热重载，并把重载前的版本压进撤销栈 —— 所以 AI 的任何改动在 GUI 里按 `Ctrl+Z` 就能撤回。GUI 里的修改 300ms 后自动保存，AI 下一次调用就能读到；若保存时发现 AI 刚写过，保留你的版本并把 AI 的版本放进撤销栈，谁的改动都不会悄悄丢。

MCP 收到的相对路径一律相对于工程所在目录（没打开工程时是 Music 目录），且只会写 `*.aidaw.json` 和 `*.wav`，AI 写错路径也覆盖不了别的文件。

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

**🐣 Easy / 🔧 Pro 开关**：窗口左上角那个大开关。Easy（新用户默认）只留必需的东西：大按钮选音色、用「心情」（Happy / Sad / Can't go wrong / Bluesy）代替调式且音符永远在调内、长度按小节算、顶栏直接有 ↩ Undo / ↪ Redo 和 ✨ Ask AI。Pro 显示全部控件：网格、声像、Solo、力度、音色微调（ADSR/滤波）、缩放。切换只改界面，不会动你的歌。

第一次打开会出现欢迎页：可以选 **2 分钟互动教程**（一步步教你播放、画音符、改音色，做对了自动进入下一步）、播放 demo、新建空白歌曲，或者按 3 步把 Claude 接进来。之后随时可以在 **❓ Help** 菜单里重新打开。

新手友好设计：乐器用预设选，内置 **78 个音色**，分成 10 组标签页：键盘（电钢、风琴、羽管键琴…）、贝斯（808、Acid、Reese…）、主音、Pad、拨弦/吉他、钟琴/马林巴、管弦（弦乐、铜管、长笛、萨克斯…）、复古 8-bit、音效（Riser、Zap…），以及 7 套鼓组（标准 / 808 / 909 / 原声 / Lo-fi / Trap / Electro，全都带牛铃、沙锤、康加等打击乐）。Pro 模式下还能调共振、滤波包络、FM、Unison、Sub、颤音、失真等；高级参数折叠起来，点钢琴键能直接试听，画音符时能听到声音；**Key + Stay in key** 会高亮调内音并把音符自动吸附过去，乱点也好听；每个按钮都有悬停说明。

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

MCP 工具（共 20 个，详见 [docs/LLM_GUIDE.md](docs/LLM_GUIDE.md)）：

- 音乐级：`add_chords`（`"Am F C G"`）、`add_drum_pattern`（`{"kick":"x...x...","snare":"....x..."}`）、`add_melody`（`"C4/1 D4/0.5 r/1 G4/2"`）—— 模型不用自己算音符
- 工程：`help`、`new_project`、`open_project`、`get_project`、`list_presets`、`update_project`、`render_wav`
- 底层编辑：`add_track` / `update_track` / `remove_track`、`add_clip` / `update_clip` / `remove_clip`、`add_notes` / `remove_notes` / `transpose_clip`、`apply_ops`（原子批量）

对小模型很宽容：轨道/clip 可以用名字引用、数字可以写成字符串、乐器可以直接写预设名（`"bass"`、`"Warm Pad"`）、位置可以用小节（`bar: 5`）；每个错误都会列出合法值并附一个示例调用。

### 任何 LLM 都能用

- **MCP 客户端**（Claude Desktop / Claude Code / Cursor / Windsurf / Cline…）：如上配置 `aidaw-mcp.exe`
- **函数调用 API**（OpenAI / Gemini / Anthropic / Ollama…）：`aidaw-mcp tools --format openai|anthropic|gemini` 导出工具定义，再用 `aidaw-mcp call <tool> '<json>' --project song.aidaw.json` 执行
- **只能跑命令的 agent**：`aidaw-mcp call add_chords --project song.aidaw.json track=Piano chords="Am F C G"`；`aidaw-mcp guide` 打印一页速查表，可以直接贴给任何聊天 AI

`examples/demo.aidaw.json` 就是通过 MCP 接口生成的 4 轨 demo。

## 路线图

- [ ] 插件宿主：VST3 / CLAP 乐器与效果器
- [ ] 音频轨与录音、采样器
- [ ] 自动化包络、效果器链（EQ / 压缩 / 混响）
- [ ] ASIO 低延迟驱动、导出 MIDI
- [ ] 循环播放区、多选/复制粘贴、量化
- [ ] 音频回调零分配、引擎与 UI 之间改为无锁命令队列
- [ ] AI 侧：`render_wav` 后回传音频分析（响度/频谱），让 AI 能“听”自己的作品
