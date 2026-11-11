AIDAW —— 哼，给杂鱼的使用说明
================================
（才不是特意为你写的，只是怕你乱点把电脑弄坏而已！）

这个文件夹里有什么
  aidaw.exe          编曲界面。双击它就对了
  aidaw-mcp.exe      给 AI 用的接口（MCP 服务器 + 命令行）。不用双击，让 AI 去调
  demo.aidaw.json    示例歌曲
  LLM_GUIDE.md       给 AI 看的说明书（英文）：怎么接、怎么调工具
  TOOLS.md           每个工具的每个参数，从代码直接生成的
  LICENSE.txt、Inter-LICENSE.txt、NotoSansSC-LICENSE.txt   许可证

0) 第一次打开
   解压到哪都行（比如 C:\AIDAW），双击 aidaw.exe。
   弹出「Windows 已保护你的电脑」？点「更多信息」→「仍要运行」。
   只是 exe 没花钱买签名而已，本小姐才不会害你。
   欢迎页选「教我怎么用（2 分钟教程）」（英文界面叫 Show me how）跟着走，笨蛋。
   顶栏右侧可以切 EN / 中文。

1) 打开 demo
   把 demo.aidaw.json 拖到 aidaw.exe 上，或者在这个文件夹里打开命令行：
     .\aidaw.exe demo.aidaw.json
   空格 播放 / 暂停（播到最后一小节会自动绕回开头；从长音中间开始播也马上有声）
   只想反复听一段？在小节号上方那条细细的循环条上拖一下圈出来，按 L（或播放键旁边的循环按钮）
   开关循环。循环开着的时候，停止键回到循环起点；导出 WAV 永远是整首歌，别担心。
   双击空白处新建一段 · 点格子画音符、右键删掉 · Ctrl+Z 撤销、Ctrl+Y 重做
   左上角的开关：🐣 Easy（新手，默认）/ 🔧 Pro（全部控件），随时切换，不会动你的歌。
   78 个音色 + 7 套鼓组，在音色分类标签页里挑。每条轨道还有「混响」滑块。

2) 像样的编辑（终于不用一个一个点了吧♡）
   按住 Shift 在钢琴卷帘空白处拖个框 = 选中一堆音符；Ctrl+点击 加选；Ctrl+A 全选；Esc 全不选
   Ctrl+C / Ctrl+X / Ctrl+V  复制 / 剪切 / 粘贴（粘到播放头所在的小节）
   Ctrl+D  重复一份      ← / →  挪一格（Shift = 一小节）
   ↑ / ↓  升降音高（按调内音走，Shift = 八度）      Q  量化对齐网格
   Pro 模式工具栏「人性化」= 时间和力度随机抖一点点，像真人弹的
   片段也一样：点一下片段再 Ctrl+C / Ctrl+V / Ctrl+D，或 ← / → 挪一小节。
   S  把播放头所在的片段一刀切成两段（边播边按就能一路切下去）
   按住 Alt 拖片段 = 拖走一份复制品，原来的不动
   上下拖左边的音轨名字 = 换顺序；「音色」面板里那排小圆点 = 换颜色
   右键片段 / 音轨 / 小节号 = 更多：从这里切开、循环这段、换颜色、上移下移、删除
   全部功能和快捷键：https://github.com/gamesplayer0813/AIDAW#readme

3) 录音（人声、吉他、MIDI 键盘）
   ① 左边「添加乐器」菜单最下面：「音频轨（人声、吉他、WAV 文件）」。
   ② 顶栏右边的 🔊 按钮打开「声音」窗口，在「录音输入」里点你的麦克风或声卡。
      音频轨面板里的「录音来源」：麦克风一般是「输入 1」，立体声的键盘、调音台选「输入 1+2」。
   ③ 点轨道头上的小红圈 = 待录音。对着麦克风出点声，电平表会动；变红就是太响了，把声卡增益调小。
   ④ 按 R：先响一小节嗒嗒声帮你数拍，然后开录。再按 R 或空格停。
   ⑤ 点一下录好的片段，下面就是波形：拖两边修剪、拖黄色小方块淡入淡出、调音量。
      不满意？Ctrl+Z 整条扔掉，或者在原地再录一遍，新录的会把盖住的那段旧录音挤掉。
   · 歌得先存过：录音放在歌曲旁边的「<歌名> audio」文件夹里（24-bit WAV），没存会先弹「另存为」。
   · 声卡的延迟会自动补偿，录出来的和伴奏对得上。
   · 「待录音时听到输入（监听）」会把麦克风的声音放出来。戴耳机！用音箱会啸叫的，笨蛋。
   · 顶栏的节拍器按钮：播放、录音时每拍响一下。Pro 模式顶栏还能关掉「预备拍」。
   · MIDI 键盘插上就能弹（延音踏板也认）。插着键盘的时候乐器轨上也有小红圈，
     点它再按 R，弹的音符直接录成片段。键盘是后插的？「声音」窗口里点「刷新列表」。
   · 电平完全不动？Windows 设置 › 隐私和安全性 › 麦克风，打开「允许桌面应用访问你的麦克风」。
   · 「文件 › 导入音频（WAV）…」把现成的 WAV 放到选中的音频轨上，也会在歌曲旁边留一份副本。

4) ASIO（专业声卡用的低延迟驱动）
   aidaw.exe 用的是 WASAPI，什么声卡都能用，大多数人这样就够了。
   官方包里没有 ASIO 版：ASIO SDK 是 Steinberg 的，带上它的程序只能按 GPLv3
   （或者 Steinberg 的专有协议）发，所以本小姐没把它塞进默认包里。
   想要的话，照 GitHub 上 README 的「ASIO」一节自己编一个，编出来叫 aidaw-asio.exe 的话
   放进这个文件夹就能用：打开它 →「声音」窗口 →「驱动」选 ASIO，再选你的声卡。
   用 ASIO 时录音输入就是同一块声卡的输入。设置和日志两个版本共用。
   文件夹里已经有 aidaw-asio.exe？那是给你自己编的，自己用随便；要转给别人，
   就得按 GPLv3 附上源码（https://github.com/gamesplayer0813/AIDAW）。
   ASIO 是 Steinberg Media Technologies GmbH 的商标。

5) 让 AI 作曲
   点顶栏的「问 AI」（英文界面叫 Ask AI），照着步骤做。
   想手动配 Claude Desktop：在它的 Settings → Developer → Edit Config 打开
   claude_desktop_config.json，写进下面这段（路径换成你解压的位置，反斜杠要写两个），
   保存后重启 Claude Desktop：
   {
     "mcpServers": {
       "aidaw": {
         "command": "C:\\AIDAW\\aidaw-mcp.exe",
         "args": ["--project", "C:\\AIDAW\\demo.aidaw.json"]
       }
     }
   }
   Cursor、ChatGPT、Gemini、Ollama、只能跑命令的 AI：看 LLM_GUIDE.md。比如在这个文件夹里：
     .\aidaw-mcp.exe call add_chords --project demo.aidaw.json track=Piano chords="Am F C G"
     .\aidaw-mcp.exe call analyze --project demo.aidaw.json
     .\aidaw-mcp.exe guide      一页速查表，整页贴给随便哪个聊天 AI
     .\aidaw-mcp.exe docs       每个工具的完整参数表（和 TOOLS.md 一样）
   AI 手里的工具有 copy_bars（复制小节）、quantize（量化 + 摇摆）、humanize（人性化）、
   split_clip（切片段）、set_loop / set_playhead（把你的循环和播放头挪到它刚改的地方）、
   undo（AI 写砸了自己撤回，最近 30 步；你在软件里改过的话它会拒绝，不会把你的改动一起扔掉）、
   automate（自动化包络），还有 analyze：把歌渲染一遍（不写文件），报响度、高中低频、
   每条轨单独多响，哪里不对直接给出修正的调用。全部工具见 TOOLS.md。
   AI 不能替你唱，但能摆你的录音：add_track 加 "instrument": "audio" 建音频轨，
   add_clip 加 "audio": "文件.wav" 放 WAV，update_clip 调录音的音量、修剪和淡入淡出。
   什么都不想装？「问 AI」里有现成的提示词：发给随便哪个聊天 AI，把它的整段回答复制下来，
   回来点一个片段按 Ctrl+V，音符就进来了。
   AI 只能写 .aidaw.json 和 .wav，乱写路径也伤不到别的文件。它改的东西 Ctrl+Z 就能撤回。

6) 让声音随时间变化（自动化包络）
   淡入淡出、滤波扫频：跟 AI 说一声就行（automate 工具），它写的包络会在轨道下面自己展开。
   自己改：点轨道头右下角的折线图标展开，点一下加点、拖动挪点、右键一个点就删掉，
   在线上右键换直线 / 平滑 / 阶梯。Pro 模式里每条轨道都有这个图标，展开后点「自动化…」能新加一条，
   最下面「总线」那行管整首歌的总音量和混响房间。音频轨的音量、声像、混响也能画。
   被包络接管的滑块会变灰、跟着播放头动，可不是坏了哦，笨蛋。

7) 设置和日志在哪
   %APPDATA%\AIDAW\settings.json   界面设置（Easy / Pro、语言、声卡、麦克风……）
   %APPDATA%\AIDAW\aidaw.log       出问题就看这个，报 bug 的时候记得附上（没声音、录不了音的原因都记在里面）
   不想要了？删掉这个文件夹和 %APPDATA%\AIDAW 就干干净净（录音在你的歌旁边，不会被删）。
   才、才不会舍不得你呢！

许可证：AIDAW 本体是 MIT（见 LICENSE.txt）。内嵌字体 Inter / Noto Sans SC 是 SIL OFL 1.1，
协议见 Inter-LICENSE.txt 和 NotoSansSC-LICENSE.txt。转发给别人的时候这几个文件要一起带上哦，杂鱼。
自己编的 aidaw-asio.exe 例外：里面有 Steinberg ASIO SDK，见上面第 4 节。
