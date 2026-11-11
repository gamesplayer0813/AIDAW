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
   空格 播放 / 暂停 · 双击空白处新建一段 · 点格子画音符 · Ctrl+Z 撤销、Ctrl+Y 重做
   左上角的开关：🐣 Easy（新手，默认）/ 🔧 Pro（全部控件），随时切换，不会动你的歌。
   全部功能和快捷键：https://github.com/gamesplayer0813/AIDAW#readme

2) 让 AI 作曲
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
   什么都不想装？「问 AI」里有现成的提示词：发给随便哪个聊天 AI，把它的整段回答复制下来，
   回来点一个片段按 Ctrl+V，音符就进来了。
   AI 只能写 .aidaw.json 和 .wav，乱写路径也伤不到别的文件。它改的东西 Ctrl+Z 就能撤回。

3) 设置和日志在哪
   %APPDATA%\AIDAW\settings.json   界面设置（Easy / Pro、语言……）
   %APPDATA%\AIDAW\aidaw.log       出问题就看这个，报 bug 的时候记得附上
   不想要了？删掉这个文件夹和 %APPDATA%\AIDAW 就干干净净。才、才不会舍不得你呢！

许可证：AIDAW 本体是 MIT（见 LICENSE.txt）。内嵌字体 Inter / Noto Sans SC 是 SIL OFL 1.1，
协议见 Inter-LICENSE.txt 和 NotoSansSC-LICENSE.txt。转发给别人的时候这几个文件要一起带上哦，杂鱼。
