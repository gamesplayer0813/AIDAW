//! English / Chinese interface text.
//!
//! The English text in the source is the key: `tr("Saved")` returns it as is
//! in English and looks up the Chinese in [`zh`] otherwise. Anything without
//! a translation falls back to English, so a missing entry never breaks the
//! UI (a test checks that every `tr`/`trf` literal has one).

use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Lang {
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh")]
    Zh,
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
}

pub fn get() -> Lang {
    if LANG.load(Ordering::Relaxed) == Lang::Zh as u8 {
        Lang::Zh
    } else {
        Lang::En
    }
}

/// The language of the operating system's interface: Chinese if the user
/// runs Windows (or a desktop) in Chinese, English otherwise.
pub fn system() -> Lang {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetUserDefaultUILanguage() -> u16;
        }
        // SAFETY: takes no arguments and only reads the user's UI language.
        let id = unsafe { GetUserDefaultUILanguage() };
        const LANG_CHINESE: u16 = 0x04;
        if id & 0x3ff == LANG_CHINESE {
            Lang::Zh
        } else {
            Lang::En
        }
    }
    #[cfg(not(windows))]
    {
        let zh = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .find(|v| !v.is_empty())
            .is_some_and(|v| v.starts_with("zh"));
        if zh {
            Lang::Zh
        } else {
            Lang::En
        }
    }
}

/// Translate a piece of interface text.
pub fn tr(en: &'static str) -> &'static str {
    match get() {
        Lang::En => en,
        Lang::Zh => zh(en).unwrap_or(en),
    }
}

/// Translate a template and fill each `{}` with the next argument.
pub fn trf(en: &'static str, args: &[&dyn Display]) -> String {
    let mut out = String::new();
    let mut args = args.iter();
    let mut parts = tr(en).split("{}");
    if let Some(first) = parts.next() {
        out.push_str(first);
    }
    for part in parts {
        if let Some(a) = args.next() {
            out.push_str(&a.to_string());
        }
        out.push_str(part);
    }
    out
}

/// Whether a status-bar message reports something that went wrong.
pub fn is_failure(status: &str) -> bool {
    let s = status.to_lowercase();
    ["failed", "couldn't", "失败", "无法"]
        .iter()
        .any(|w| s.contains(w))
}

fn zh(en: &str) -> Option<&'static str> {
    Some(match en {
        // ---- language switch ----
        "Language: English / 中文" => "语言：English / 中文",
        "Language: English" => "语言：English",
        "Language: 中文" => "语言：中文",

        // ---- starter song ----
        "My Song" => "我的歌曲",
        "Piano" => "钢琴",
        "Drums" => "鼓",
        "Melody" => "旋律",
        "Beat" => "节拍",
        "Part {}" => "段落 {}",

        // ---- status bar ----
        "Opened {}" => "已打开 {}",
        "Failed to open: {}" => "打开失败：{}",
        "New project, will save to {}" => "新项目，将保存到 {}",
        "Welcome! Not sure where to start? Open ❓ Help › Tutorial." => {
            "欢迎！不知道从哪开始？打开 ❓ 帮助 › 教程。"
        }
        "No sound device. Opening the Sound window so you can pick one." => {
            "没有声音设备。正在打开「声音」窗口，请选一个。"
        }
        "Sound now plays through {}" => "现在通过 {} 播放声音",
        "Couldn't open that sound device" => "无法打开这个声音设备",
        "The sound device isn't running. Pick another one in the Sound window." => {
            "声音设备没有运行。请在「声音」窗口里换一个。"
        }
        "Edit failed: {}" => "编辑失败：{}",
        "Undone (Ctrl+Y to redo)" => "已撤销（Ctrl+Y 重做）",
        "Redone" => "已重做",
        "The AI changed the song while you were editing; kept your version (Ctrl+Z shows the AI's)" => {
            "你编辑时 AI 也改了歌曲；已保留你的版本（按 Ctrl+Z 查看 AI 的版本）"
        }
        "Save failed: {}" => "保存失败：{}",
        "Reloaded external change (Ctrl+Z to revert)" => "已载入外部修改（Ctrl+Z 撤回）",
        "Open failed: {}" => "打开失败：{}",
        "Saving to {} (AI tools can now use this file)" => {
            "保存到 {}（AI 工具现在可以使用这个文件）"
        }
        "An export is already running" => "已经有一个导出在进行中",
        "Exporting audio… {}%" => "正在导出音频… {}%",
        "Exported {}s to {}" => "已导出 {} 秒音频到 {}",
        "Export failed: {}" => "导出失败：{}",
        "the export stopped unexpectedly" => "导出意外中止",
        "Saved" => "已保存",
        "New song. Click in the piano roll to add notes!" => {
            "新歌曲。在钢琴卷帘里点击即可添加音符！"
        }
        "Demo song loaded. Change anything you like!" => "示例歌曲已载入，随便改吧！",
        "The project file the AI edits through aidaw-mcp; changes appear here live." => {
            "AI 通过 aidaw-mcp 编辑的项目文件；修改会实时显示在这里。"
        }
        "grid {} · key {} {}" => "网格 {} · 调 {} {}",
        "Easy mode · switch to Pro (top left) for more controls" => "简单模式 · 切换到左上角的专业模式可显示更多控件",
        "Deleted {}. Ctrl+Z (or ↩ Undo) brings it back." => {
            "已删除 {}。按 Ctrl+Z（或 ↩ 撤销）可恢复。"
        }
        "Copied! Paste it into Claude Desktop's config file." => {
            "已复制！把它粘贴到 Claude Desktop 的配置文件里。"
        }

        // ---- files ----
        "AIDAW project" => "AIDAW 项目",
        "WAV audio" => "WAV 音频",

        // ---- top bar ----
        "Ask AI" => "问 AI",
        "Let an AI assistant (Claude, ChatGPT, …) write music into this song" => {
            "让 AI 助手（Claude、ChatGPT……）直接往这首歌里写音乐"
        }
        "File" => "文件",
        "New song" => "新建歌曲",
        "Open…" => "打开…",
        "Save As…  (Ctrl+S)" => "另存为…  (Ctrl+S)",
        "Export as WAV audio…" => "导出为 WAV 音频…",
        "Save as audio file…" => "保存为音频文件…",
        "Make an audio file you can share or put in other apps" => {
            "生成一个可以分享或导入其他软件的音频文件"
        }
        "Help" => "帮助",
        "Tutorial (2 minutes)" => "教程（2 分钟）",
        "Let AI make music…" => "让 AI 来作曲…",
        "Sound settings / no sound?" => "声音设置 / 没有声音？",
        "⌨ Keyboard & mouse" => "⌨ 键盘和鼠标",
        "Keyboard & mouse" => "键盘和鼠标",
        "Welcome screen" => "欢迎界面",
        "Undo" => "撤销",
        "Take back the last change (Ctrl+Z). Works for AI changes too." => {
            "撤回上一次修改（Ctrl+Z），AI 做的修改也能撤回。"
        }
        "Redo" => "重做",
        "Redo (Ctrl+Y)" => "重做（Ctrl+Y）",
        "Grid" => "网格",
        "Notes and clips snap to this grid. 1/16 is a good default; 1/4 = one beat." => {
            "音符和片段会对齐到这个网格。1/16 是不错的默认值；1/4 = 一拍。"
        }
        "{} beat" => "{} 拍",
        "Not saved. Click to save" => "未保存，点击保存",
        "Save your song to a file. AI tools work on this file too." => {
            "把歌曲保存成文件，AI 工具也在这个文件上工作。"
        }
        "Stop and go back to the start" => "停止并回到开头",
        "Pause" => "暂停",
        "Play" => "播放",
        "Play or pause the song (Space bar)" => "播放或暂停歌曲（空格键）",
        "BAR" => "小节",
        "Where the song is playing: bar number . beat" => "当前播放位置：小节号 . 拍",
        "SPEED" => "速度",
        "Tempo in beats per minute. 70 = slow ballad, 110 = pop, 170 = drum & bass." => {
            "速度，单位是每分钟拍数。70 = 慢情歌，110 = 流行，170 = 鼓打贝斯。"
        }
        "How fast the song plays (beats per minute).\n70 = slow and calm · 110 = pop · 140+ = fast and energetic." => {
            "歌曲播放的快慢（每分钟拍数）。\n70 = 舒缓平静 · 110 = 流行 · 140+ = 快速有活力。"
        }
        "Drag left/right or click to type" => "左右拖动或点击输入",
        "Slower" => "慢一点",
        "Faster" => "快一点",
        "Output level" => "输出电平",
        "🔊 Sound" => "🔊 声音",
        "Audio" => "声音",
        "Sound settings: pick speakers, play a test beep" => "声音设置：选择扬声器、播放测试音",
        "No sound. Click to fix" => "没有声音，点击修复",

        // ---- Easy / Pro ----
        "Easy" => "简单",
        "Pro" => "专业",
        "Easy mode: only the essentials, big buttons, everything stays in key.\nYou can switch any time; your song is not changed." => {
            "简单模式：只显示必需的控件，按钮更大，音符始终在调内。\n随时可以切换，歌曲不会被改动。"
        }
        "Pro mode: every control (grid, pan, solo, note velocity, sound shaping, zoom…).\nYou can switch any time; your song is not changed." => {
            "专业模式：显示所有控件（网格、声像、独奏、音符力度、音色调节、缩放……）。\n随时可以切换，歌曲不会被改动。"
        }
        "Switched to Pro mode: every control is shown. Your song is unchanged." => "已切换到专业模式：显示所有控件，歌曲没有变化。",
        "Switched to Easy mode: just the essentials. Your song is unchanged." => "已切换到简单模式：只显示必需的控件，歌曲没有变化。",

        // ---- side bar ----
        "Tracks" => "音轨",
        "Instruments" => "乐器",
        "Add instrument" => "添加乐器",
        "Pick a sound for the new track" => "为新音轨选一个音色",
        "Add a new track. Each track plays one instrument." => {
            "添加一条新音轨，每条音轨演奏一种乐器。"
        }
        "No tracks yet. Click + Add instrument above to start." => "还没有音轨。点击上方的 + 添加乐器开始吧。",
        "Select this track to change its sound" => "选中这条音轨来更换音色",
        "Delete this track (Ctrl+Z brings it back)" => "删除这条音轨（Ctrl+Z 可恢复）",
        "Solo: hear ONLY this track" => "独奏：只听这条音轨",
        "Mute: silence this track" => "静音：让这条音轨不出声",
        "🔇 Silence" => "🔇 静音",
        "Turn this instrument off for now" => "暂时关掉这个乐器",
        "Volume" => "音量",
        "Volume: how loud this track is (1.0 = unchanged)" => {
            "音量：这条音轨有多响（1.0 = 原始音量）"
        }
        "L/R" => "左/右",
        "Pan" => "声像",
        "Pan: move the sound to the left or right speaker" => {
            "声像：把声音移向左边或右边的扬声器"
        }
        "mid" => "居中",
        "Centre the pan" => "把声像调回中间",
        "How loud this instrument is" => "这个乐器有多响",
        "Master volume" => "总音量",
        "Volume of the whole song" => "整首歌的音量",
        "Reverb" => "混响",
        "Echo of a room around this sound: 0 = dry and close, more = farther away and more spacious" => "给声音加上房间的回声：0 = 干、离你很近，越大越远、越有空间感",
        "Reverb room" => "混响房间",
        "Size of the shared reverb: small room .. big hall" => "所有轨道共用的混响有多大：小房间 .. 大音乐厅",
        "Reverb tone" => "混响音色",
        "Dark, soft echoes .. bright, airy ones" => "暗、柔和的回声 .. 明亮、通透的回声",
        "Click a track above to change its sound." => "点击上方的音轨来更换音色。",
        "Sound" => "音色",
        "Track name" => "音轨名称",
        "⚙ Custom" => "⚙ 自定义",
        "Pick a sound (click to hear it):" => "选一个音色（点击试听）：",
        "🔊 Try" => "🔊 试听",
        "Hear this sound" => "听听这个音色",
        "⚙ Fine-tune the sound" => "⚙ 微调音色",
        "Wave" => "波形",
        "The basic tone colour" => "基本音色",
        "Fade in" => "淡入",
        "Attack: how slowly the note starts" => "起音（Attack）：音符开始得有多慢",
        "Drop" => "衰减",
        "Decay: how fast it falls to the hold level" => "衰减（Decay）：多快降到保持电平",
        "Hold level" => "保持电平",
        "Sustain: loudness while the note is held" => "延音（Sustain）：按住音符时的响度",
        "Fade out" => "淡出",
        "Release: how long it rings after the note ends" => {
            "释音（Release）：音符结束后余音持续多久"
        }
        "Brightness" => "亮度",
        "Filter cutoff: lower = darker, softer" => "滤波截止频率：越低越暗、越柔和",
        "Each row in the piano roll is a different drum: Kick, Snare, Clap, Hi-hats, Toms and Crash are labelled on the keyboard." => {
            "钢琴卷帘里每一行是一种不同的鼓：底鼓、军鼓、拍手、踩镲、通鼓和吊镲都标在键盘上。"
        }
        "sine" => "正弦波",
        "triangle" => "三角波",
        "saw" => "锯齿波",
        "square" => "方波",

        // ---- presets ----
        "🎹 Soft Keys" => "🎹 柔和键盘",
        "A gentle piano-like sound. Good for chords and melodies." => {
            "温柔的类钢琴音色，适合和弦与旋律。"
        }
        "🎸 Bass" => "🎸 贝斯",
        "Deep and punchy. Use low notes (around C2)." => "低沉有力，用低音（C2 附近）。",
        "☁ Warm Pad" => "☁ 温暖铺底",
        "Slow, soft and wide. Hold long chords with it." => "缓慢、柔和、宽广，适合长和弦。",
        "⚡ Bright Lead" => "⚡ 明亮主音",
        "Cuts through the mix. Great for the main melody." => "穿透力强，很适合主旋律。",
        "💧 Pluck" => "💧 拨弦",
        "Short and bouncy. Try fast repeating notes." => "短促有弹性，试试快速重复的音。",
        "🔔 Bell" => "🔔 铃声",
        "Clear, ringing tone. Nice for high sparkly notes." => "清脆响亮，适合高音点缀。",
        "👾 8-bit" => "👾 8-bit 芯片",
        "Retro video game sound." => "复古电子游戏音色。",
        "〰 Pure Tone" => "〰 纯音",
        "A plain, smooth sine wave." => "平滑干净的正弦波。",
        "💥 Drum Kit" => "💥 架子鼓",
        "Kick, snare, hi-hats and more. Each row is a different drum." => {
            "底鼓、军鼓、踩镲等等，每一行是一种不同的鼓。"
        }

        "🎹 Bright Piano" => "🎹 明亮钢琴",
        "A brighter, punchier piano. Cuts through a busy song." => "更亮、更有冲击力的钢琴，在热闹的歌里也听得清。",
        "🎹 Electric Piano" => "🎹 电钢琴",
        "Warm, bell-ish 70s electric piano. Lovely for soul, jazz and lo-fi chords." => "温暖、带点铃声感的 70 年代电钢琴，很适合灵魂乐、爵士和 Lo-fi 和弦。",
        "🎹 Wurly" => "🎹 簧片电钢",
        "Reedy, slightly gritty electric piano for funk and indie." => "带簧片味、略带颗粒感的电钢琴，适合放克和独立音乐。",
        "⛪ Organ" => "⛪ 电风琴",
        "Rock and gospel drawbar organ. Hold chords; it never fades." => "摇滚和福音风格的拉杆风琴。按住和弦，声音不会衰减。",
        "⛪ Church Organ" => "⛪ 教堂管风琴",
        "Big, hollow pipe organ. Slow chords sound grand." => "宏大空灵的管风琴，慢和弦听起来很庄严。",
        "🎹 Clavinet" => "🎹 克拉维内特",
        "Snappy, funky keyboard. Play short rhythmic notes." => "清脆带劲的放克键盘，弹短促有节奏的音。",
        "🎼 Harpsichord" => "🎼 羽管键琴",
        "Baroque, plucked-string keyboard." => "巴洛克风格的拨弦键盘乐器。",
        "📀 Lo-fi Keys" => "📀 Lo-fi 键盘",
        "Dusty, wobbly electric piano, like an old tape. Chill beats." => "像旧磁带一样带灰尘感、微微摇晃的电钢琴，适合放松的节拍。",
        "🔈 Sub Bass" => "🔈 超低音贝斯",
        "Pure, deep low end you feel more than hear. C1-C2." => "纯净深沉的低频，与其说听到不如说感受到。用 C1-C2。",
        "💣 808 Bass" => "💣 808 贝斯",
        "Booming hip-hop/trap bass with a punchy pitch drop. Long notes around C1-C2." => "轰鸣的嘻哈 / Trap 贝斯，开头音高有力下滑。在 C1-C2 附近弹长音。",
        "🎸 Synth Bass" => "🎸 合成器贝斯",
        "Fat analog bass with a little 'wow' on every note. Funk, pop, synthwave." => "肥厚的模拟贝斯，每个音都有一点「哇」声。适合放克、流行、合成器浪潮。",
        "🐍 Acid Bass" => "🐍 Acid 贝斯",
        "Squelchy 303 acid line. Fast 16th notes around C2." => "叽叽喳喳的 303 酸性旋律，在 C2 附近弹快速十六分音符。",
        "🐉 Reese Bass" => "🐉 Reese 贝斯",
        "Wide, growling detuned bass for drum & bass and dubstep." => "宽厚、低吼的失谐贝斯，适合鼓打贝斯和回响贝斯。",
        "🎸 Pluck Bass" => "🎸 拨弦贝斯",
        "Short, bouncy bass for house and pop." => "短促有弹性的贝斯，适合浩室和流行。",
        "🎸 Finger Bass" => "🎸 指弹贝斯",
        "Round electric bass guitar. Rock, funk, pop." => "圆润的电贝斯，适合摇滚、放克、流行。",
        "🔥 Distorted Bass" => "🔥 失真贝斯",
        "Aggressive, fuzzy bass for rock, dubstep and metal." => "凶猛的毛刺贝斯，适合摇滚、回响贝斯和金属。",
        "🚀 Supersaw" => "🚀 超级锯齿",
        "Huge, shiny stack of saws. Trance and EDM anthems." => "巨大闪亮的多层锯齿波，迷幻舞曲和 EDM 必备。",
        "⭐ Analog Lead" => "⭐ 模拟主音",
        "Classic singing mono-synth lead with vibrato." => "经典的带颤音、会「唱歌」的单音合成器主音。",
        "🎮 Square Lead" => "🎮 方波主音",
        "Hollow, woody lead. Melodies that sound playful." => "空心、木质感的主音，旋律听起来很俏皮。",
        "📻 Pulse Lead" => "📻 脉冲主音",
        "Thin, nasal and bright lead. Synth-pop melodies." => "纤细、带鼻音的明亮主音，适合合成器流行旋律。",
        "🌙 Soft Lead" => "🌙 柔和主音",
        "Smooth, whistle-like lead. Gentle melodies." => "平滑、像口哨一样的主音，适合温柔的旋律。",
        "🔥 Hard Lead" => "🔥 硬核主音",
        "Screaming, distorted lead. Rock solos and big drops." => "尖叫的失真主音，适合摇滚独奏和大 Drop。",
        "🌌 Lush Pad" => "🌌 华丽铺底",
        "Huge, shimmering wall of sound. Big emotional chords." => "巨大闪烁的声墙，适合饱含情绪的大和弦。",
        "💎 Glass Pad" => "💎 玻璃铺底",
        "Clear, crystal-like pad. Dreamy and bright." => "清澈如水晶的铺底，梦幻又明亮。",
        "🌃 Dark Pad" => "🌃 暗色铺底",
        "Low, moody, cinematic. Great under sad or tense music." => "低沉、阴郁、有电影感，适合垫在伤感或紧张的音乐下面。",
        "🎤 Choir" => "🎤 合唱",
        "Soft 'aah' voices. Hold chords for an angelic sound." => "柔和的「啊」人声，按住和弦就像天使在唱。",
        "💨 Airy Pad" => "💨 空气铺底",
        "Breathy, floating pad with a soft hiss." => "带气声、漂浮感的铺底，有轻柔的嘶嘶声。",
        "🌊 Sweep Pad" => "🌊 扫频铺底",
        "Each chord slowly opens up like a wave." => "每个和弦都像海浪一样慢慢展开。",
        "✨ Sparkle Pluck" => "✨ 闪亮拨弦",
        "Bright, wide pluck for future bass and pop arpeggios." => "明亮宽广的拨弦，适合未来贝斯和流行琶音。",
        "🎸 Nylon Guitar" => "🎸 尼龙弦吉他",
        "Soft acoustic guitar. Strum chords or pick melodies." => "柔和的木吉他，扫和弦或拨旋律都行。",
        "🎸 Clean Guitar" => "🎸 清音电吉他",
        "Electric guitar without distortion. Funk, indie, pop." => "不加失真的电吉他，适合放克、独立、流行。",
        "🔥 Rock Guitar" => "🔥 摇滚吉他",
        "Crunchy distorted guitar. Power chords (root + fifth) rock!" => "有颗粒感的失真吉他，强力和弦（根音 + 五度）超带劲！",
        "🌸 Harp" => "🌸 竖琴",
        "Graceful harp. Lovely for flowing arpeggios." => "优雅的竖琴，很适合流动的琶音。",
        "🍀 Koto" => "🍀 日本筝",
        "Twangy Japanese zither. Try the pentatonic scale." => "弦音铮铮的日本筝，试试五声音阶。",
        "🎐 Music Box" => "🎐 八音盒",
        "Tiny, tinkly music box. Sweet lullabies, high notes." => "小巧叮咚的八音盒，适合甜美的摇篮曲和高音。",
        "🔨 Marimba" => "🔨 马林巴",
        "Warm wooden mallets. Tropical and playful." => "温暖的木质琴槌声，热带又俏皮。",
        "♦ Xylophone" => "♦ 木琴",
        "Bright, dry and short mallet sound." => "明亮、干脆、短促的敲击音色。",
        "🎐 Vibraphone" => "🎐 颤音琴",
        "Smooth jazz vibes with a gentle shimmer." => "带轻柔闪烁的爵士颤音琴。",
        "✨ Glockenspiel" => "✨ 钟琴",
        "Sparkly metal bars. Christmas and fairy-tale melodies." => "闪闪发光的金属片，适合圣诞和童话旋律。",
        "💧 Kalimba" => "💧 卡林巴",
        "Soft thumb piano. Calm, cozy melodies." => "柔和的拇指琴，适合安静温馨的旋律。",
        "🔔 Church Bells" => "🔔 教堂钟",
        "Big, long, clangy bells. Use sparingly for drama." => "又大又长的铛铛钟声，少量使用来制造戏剧感。",
        "🌴 Steel Drum" => "🌴 钢鼓",
        "Sunny Caribbean steel pan." => "阳光灿烂的加勒比钢鼓。",
        "🎻 Strings" => "🎻 弦乐",
        "Lush string section. Film-score chords and long notes." => "华丽的弦乐组，适合电影配乐的和弦和长音。",
        "🎻 Cello" => "🎻 大提琴",
        "Deep, expressive solo string. Slow melodies around C3." => "深沉、富有表现力的独奏弦乐，在 C3 附近弹慢旋律。",
        "🎻 Pizzicato" => "🎻 拨奏弦乐",
        "Plucked strings. Sneaky, playful, cartoonish." => "拨弦奏法，鬼鬼祟祟、俏皮、卡通感。",
        "🎺 Brass Section" => "🎺 铜管组",
        "Bold, punchy horns. Stabs and fanfares." => "大胆有力的铜管，适合短促重音和号角。",
        "🎺 Trumpet" => "🎺 小号",
        "Bright solo trumpet. Melodies around C4-C5." => "明亮的独奏小号，在 C4-C5 附近弹旋律。",
        "📯 French Horn" => "📯 圆号",
        "Mellow, heroic horn. Big film themes." => "圆润、英雄气概的圆号，适合宏大的电影主题。",
        "🎷 Saxophone" => "🎷 萨克斯",
        "Breathy, soulful sax. Jazz and smooth melodies." => "带气声、有灵魂的萨克斯，适合爵士和顺滑的旋律。",
        "🎵 Flute" => "🎵 长笛",
        "Airy, sweet flute. High melodies around C5." => "轻盈甜美的长笛，在 C5 附近弹高音旋律。",
        "🎵 Pan Flute" => "🎵 排箫",
        "Breathy wooden pipes. Folk and world music." => "带气声的木管排箫，适合民谣和世界音乐。",
        "🎶 Clarinet" => "🎶 单簧管",
        "Woody, hollow reed. Warm melodies." => "木质、空心的簧管音色，适合温暖的旋律。",
        "🎶 Oboe" => "🎶 双簧管",
        "Nasal, singing reed. Pastoral, slightly sad melodies." => "带鼻音、会歌唱的簧管，适合田园、略带忧伤的旋律。",
        "👾 NES Pulse" => "👾 红白机脉冲",
        "Thin console-style pulse. Fast arpeggios and melodies." => "纤细的游戏机脉冲波，适合快速琶音和旋律。",
        "🕹 NES Triangle" => "🕹 红白机三角波",
        "The soft console triangle, usually the bass line." => "柔和的游戏机三角波，通常用来弹贝斯线。",
        "🎮 Game Lead" => "🎮 游戏主音",
        "Wobbly arcade hero melody." => "摇摇晃晃的街机英雄旋律。",
        "🌆 Synthwave" => "🌆 合成器浪潮",
        "80s analog polysynth. Neon nights and car chases." => "80 年代模拟复音合成器，霓虹夜晚和飞车追逐。",
        "🚀 Riser" => "🚀 上升音效",
        "Swells up into the note. Put one long note before a drop." => "声音一路涨上去，在 Drop 之前放一个长音。",
        "💫 Zap" => "💫 激光",
        "Laser 'pew'. Short notes, any pitch." => "激光「咻」声，短音，任何音高都行。",
        "💣 Boom" => "💣 轰隆",
        "Cinematic impact. One low note on the big moment." => "电影感的撞击声，在高潮时刻放一个低音。",
        "💨 Wind" => "💨 风声",
        "Whooshing noise. Pitch doesn't matter, note length does." => "呼呼的风声。音高不重要，音符长度才重要。",
        "👻 Theremin" => "👻 特雷门琴",
        "Spooky wobbly sci-fi tone." => "诡异摇晃的科幻音色。",
        "🔮 Magic" => "🔮 魔法",
        "Glittering fairy-dust sparkle. High notes." => "闪闪发光的仙尘音效，用高音。",
        "💣 808 Drums" => "💣 808 鼓机",
        "The legendary hip-hop drum machine: boomy kick, snappy snare, cowbell." => "传奇嘻哈鼓机：轰鸣的底鼓、清脆的军鼓、牛铃。",
        "🎧 909 Drums" => "🎧 909 鼓机",
        "Punchy house and techno drum machine. Four-on-the-floor!" => "有力的浩室和科技舞曲鼓机，四拍底鼓走起！",
        "🎪 Acoustic Drums" => "🎪 原声架子鼓",
        "A real-sounding drum set for rock, pop and jazz." => "听起来像真鼓的架子鼓，适合摇滚、流行和爵士。",
        "📀 Lo-fi Drums" => "📀 Lo-fi 鼓",
        "Dusty, crunchy drums for chill beats." => "带灰尘感、颗粒感的鼓，适合放松的节拍。",
        "🔥 Trap Drums" => "🔥 Trap 鼓",
        "Long sub kick and crisp hats. Try fast hi-hat rolls." => "长长的超低音底鼓和清脆的踩镲，试试快速踩镲滚奏。",
        "⚡ Electro Drums" => "⚡ 电子鼓机",
        "Hard, gritty, distorted drum machine." => "坚硬、粗粝、失真的鼓机。",

        // ---- preset categories, drum kits ----
        "🎹 Keys" => "🎹 键盘",
        "⚡ Lead" => "⚡ 主音",
        "☁ Pads" => "☁ 铺底",
        "💧 Plucks" => "💧 拨弦",
        "🔔 Bells" => "🔔 钟琴",
        "🎻 Orchestra" => "🎻 管弦",
        "👾 Retro" => "👾 复古",
        "🌀 FX" => "🌀 音效",
        "💥 Drums" => "💥 鼓",
        "standard" => "标准",
        "808" => "808",
        "909" => "909",
        "acoustic" => "原声",
        "lofi" => "Lo-fi",
        "trap" => "Trap",
        "electro" => "电子",
        "pulse" => "脉冲波",
        "noise" => "噪声",

        // ---- more sound shaping ----
        "🔊 More sound shaping" => "🔊 更多音色塑形",
        "Resonance" => "共振",
        "A whistling peak at the brightness point (acid, wah)" => "在亮度点上的尖锐峰值（Acid、哇音）",
        "Filter sweep" => "滤波扫频",
        "How much brighter each note starts, in octaves (plucks, brass)" => "每个音开头亮多少，单位是八度（拨弦、铜管）",
        "Sweep time" => "扫频时间",
        "How fast the filter sweep closes again" => "滤波扫频多快收回去",
        "Voices" => "声部数",
        "Unison: stack detuned copies for a wider, fatter sound" => "齐奏：叠加略微失谐的副本，声音更宽更厚",
        "Detune" => "失谐",
        "How far apart the stacked voices are, in cents" => "叠加声部之间相差多少，单位是音分",
        "Sub" => "副低音",
        "Add a sine one octave lower: more weight" => "加一个低八度的正弦波，更有分量",
        "Noise" => "噪声",
        "Mix in hiss: breath, air, grit" => "混入嘶嘶声：气息、空气感、颗粒感",
        "FM" => "FM",
        "Bell / electric-piano tone (sine and triangle waves)" => "钟声 / 电钢琴音色（正弦波和三角波）",
        "FM ratio" => "FM 比例",
        "1 = warm, 2 = hollow, 3.5+ = metallic bell" => "1 = 温暖，2 = 空心，3.5 以上 = 金属钟声",
        "FM fade" => "FM 衰减",
        "How fast the FM tone fades (0 = never)" => "FM 音色多快消失（0 = 不消失）",
        "Vibrato" => "颤音",
        "Wobble in the pitch, in cents" => "音高的摇晃幅度，单位是音分",
        "Vibrato speed" => "颤音速度",
        "Pitch drop" => "音高下滑",
        "Start this many semitones higher and slide down (808, zaps)" => "从高这么多半音的地方开始往下滑（808、激光）",
        "Drop time" => "下滑时间",
        "Drive" => "过载",
        "Warmth at low values, distortion at high ones" => "数值低时温暖，数值高时失真",
        "Kit" => "鼓组",
        "Each row in the piano roll is a different drum: Kick, Snare, Clap, Hi-hats, Toms, cymbals and percussion (cowbell, shaker, congas...) are labelled on the keyboard." => {
            "钢琴卷帘里每一行是一种不同的鼓：底鼓、军鼓、拍手、踩镲、通鼓、镲片和打击乐（牛铃、沙锤、康加鼓……）都标在键盘上。"
        }
        "Rim" => "鼓边",
        "Ride" => "叮叮镲",
        "Ride bell" => "叮叮镲帽",
        "Tamb." => "铃鼓",
        "Splash" => "水镲",
        "Cowbell" => "牛铃",
        "Bongo H" => "高邦戈",
        "Bongo L" => "低邦戈",
        "Conga M" => "闷康加",
        "Conga H" => "高康加",
        "Conga L" => "低康加",
        "Timb. H" => "高天巴",
        "Timb. L" => "低天巴",
        "Agogo H" => "高阿哥哥",
        "Agogo L" => "低阿哥哥",
        "Shaker" => "沙锤",
        "Claves" => "响棒",
        "Block H" => "高木鱼",
        "Block L" => "低木鱼",
        "Triangle" => "三角铁",

        // ---- arrangement ----
        "Your song" => "你的歌曲",
        "Each row is one instrument. The coloured blocks are parts of the song.\nClick a part to see and edit its notes below." => {
            "每一行是一种乐器，彩色方块是歌曲的段落。\n点击一个段落，就能在下方查看和编辑它的音符。"
        }
        "click a part to edit its notes · double-click an empty spot for a new part · drag to move · Ctrl+D repeats the part" => {
            "点击段落编辑音符 · 双击空白处新建段落 · 拖动来移动 · Ctrl+D 重复这个段落"
        }
        "Song" => "歌曲",
        "Each row is one instrument (a track). The coloured blocks are clips: pieces of music.\nClick a clip to edit its notes in the piano roll below." => {
            "每一行是一种乐器（音轨），彩色方块是片段：一小段音乐。\n点击片段，就能在下方的钢琴卷帘里编辑它的音符。"
        }
        "zoom" => "缩放",
        "click a clip to edit it · double-click an empty spot for a new clip · drag to move · drag the right edge to make longer · Ctrl+D duplicates" => {
            "点击片段进行编辑 · 双击空白处新建片段 · 拖动来移动 · 拖右边缘来加长 · Ctrl+D 复制一份"
        }
        "Double-click here to add a clip" => "双击这里添加片段",
        "No tracks yet. Click + Add instrument on the left." => "还没有音轨。点击左边的 + 添加乐器。",
        "Custom sound" => "自定义音色",

        // ---- piano roll ----
        "👆 Click a coloured clip above to see and edit its notes here.\nNo clips yet? Double-click an empty spot in a track row." => {
            "👆 点击上方的彩色片段，就能在这里查看和编辑它的音符。\n还没有片段？在音轨行的空白处双击。"
        }
        "Click in the grid to add drum hits.\nTry Kick on beats 1 and 3, Snare on 2 and 4, Hi-hat everywhere!" => {
            "在网格里点击添加鼓点。\n试试在第 1、3 拍放底鼓，第 2、4 拍放军鼓，踩镲每拍都放！"
        }
        "This clip is empty. Click anywhere in the grid to add a note!\nHold a piano key on the left to hear it, slide up or down to hear others." => {
            "这个片段是空的。在网格里随便点一下就能添加音符！\n按住左边的琴键可以试听，上下滑动听其他音。"
        }
        "Notes" => "音符",
        "Each row is a piano key (low at the bottom, high at the top). Left to right is time." => {
            "每一行是一个琴键（下面低、上面高），从左到右是时间。"
        }
        "Part" => "段落",
        "Length" => "长度",
        "How many bars this part lasts" => "这个段落持续几个小节",
        " bar" => " 小节",
        " bars" => " 小节",
        "Lower" => "降八度",
        "Play every note in this part one octave lower" => "把这个段落的所有音符降低一个八度",
        "Higher" => "升八度",
        "Play every note in this part one octave higher" => "把这个段落的所有音符升高一个八度",
        "Clear" => "清空",
        "Remove every note in this part (Ctrl+Z to undo)" => {
            "删除这个段落的所有音符（Ctrl+Z 撤销）"
        }
        "Note {}" => "音符 {}",
        "Soft" => "轻",
        "Medium" => "中",
        "Loud" => "重",
        "How hard this note is played" => "这个音弹得多重",
        "🎵 Mood" => "🎵 情绪",
        "Pick a feeling. Lit-up rows fit the mood, and every note you draw lands on one of them, so it always sounds right." => {
            "选一种感觉。亮起的行符合这种情绪，你画的每个音符都会自动落在这些行上，所以怎么画都好听。"
        }
        "😊 Happy" => "😊 开心",
        "C major: bright and cheerful" => "C 大调：明亮欢快",
        "😢 Sad" => "😢 伤感",
        "A minor: dark and emotional" => "A 小调：深沉动情",
        "👌 Can't go wrong" => "👌 怎么弹都对",
        "C pentatonic: almost anything you draw sounds nice" => "C 五声音阶：几乎怎么画都好听",
        "🎷 Bluesy" => "🎷 布鲁斯",
        "A blues: cool and gritty" => "A 布鲁斯音阶：酷而粗粝",
        "Drums don't have a mood: every row is a different drum" => {
            "鼓没有情绪之分：每一行都是不同的鼓"
        }
        "How hard the selected notes are played" => "选中的音符弹得多重",
        "click to add a note · drag to move · drag the right edge to make longer · right-click to delete · hold a piano key to hear it · Shift+drag to select several" => {
            "点击添加音符 · 拖动来移动 · 拖右边缘来加长 · 右键删除 · 按住琴键试听 · 按住 Shift 拖动可以框选多个音符"
        }
        "Piano roll" => "钢琴卷帘",
        "Clip" => "片段",
        "How long this clip is. 4 beats = 1 bar." => "这个片段有多长。4 拍 = 1 小节。",
        " beats" => " 拍",
        "Move all notes" => "移动全部音符",
        "Transpose: shift every note in this clip up or down" => {
            "移调：把这个片段的所有音符整体升高或降低"
        }
        "Down one octave" => "降一个八度",
        "Down a little (one semitone)" => "降一点（一个半音）",
        "Up a little (one semitone)" => "升一点（一个半音）",
        "Up one octave" => "升一个八度",
        "Remove every note in this clip (Ctrl+Z to undo)" => {
            "删除这个片段的所有音符（Ctrl+Z 撤销）"
        }
        "Loudness" => "力度",
        "Velocity: how hard the note is played" => "力度（Velocity）：这个音弹得多重",
        "Note length in beats" => "音符长度（拍）",
        "🎵 Key" => "🎵 调",
        "Notes in the same key sound good together. Lit-up rows are in the key." => {
            "同一个调里的音放在一起很好听。亮起的行就是调内音。"
        }
        "Stay in key" => "保持在调内",
        "New and moved notes jump to the nearest note in the key, so it always sounds right." => {
            "新加和移动的音符会自动跳到最近的调内音，所以总是好听。"
        }
        "Drums don't have a key" => "鼓没有调",
        "Quantize" => "量化",
        "Snap note starts to the grid (Q): the selected notes, or every note in the clip when none is selected" => {
            "把音符的开头对齐到网格（Q）：有选中的就只对齐选中的，没有就对齐整个片段"
        }
        "click to add a note · drag to move, right edge to lengthen · right-click deletes · Shift+drag selects · Ctrl+C / V / D copy, paste, repeat · Q quantizes" => {
            "点击添加音符 · 拖动来移动，拖右边缘来加长 · 右键删除 · Shift+拖动框选 · Ctrl+C / V / D 复制、粘贴、重复 · Q 量化"
        }

        // ---- editing keys ----
        "1 note" => "1 个音符",
        "{} notes" => "{} 个音符",
        "Select some notes first: click one, or Shift+drag around several" => {
            "先选中音符：点一个，或者按住 Shift 拖框选中几个"
        }
        "Click a clip first, then paste the notes into it" => "先点一个片段，再把音符粘贴进去",
        "Add an instrument first (+ Add instrument on the left)" => {
            "先添加一个乐器（左边的 + 添加乐器）"
        }
        "Pasted the clip" => "已粘贴片段",
        "Nothing to paste: copy some notes or a clip first (Ctrl+C)" => {
            "没有可粘贴的东西：先复制一些音符或一个片段（Ctrl+C）"
        }
        "Already on the grid" => "已经都在网格上了",
        "The notes can't move before the start of the clip" => "音符不能移到片段开头之前",
        "Cut the clip (Ctrl+V pastes it at the playhead)" => "已剪切片段（Ctrl+V 粘贴到播放头处）",
        "Copied the clip (Ctrl+V pastes it at the playhead)" => "已复制片段（Ctrl+V 粘贴到播放头处）",
        "Duplicated the clip (Ctrl+D again for another copy)" => {
            "已复制出一个片段（再按 Ctrl+D 再来一份）"
        }
        "That spot was taken, so the copy went to bar {}" => {
            "那里已经有片段了，复制的片段放到了第 {} 小节"
        }
        "Cut {} (Ctrl+V pastes at the playhead)" => "已剪切 {}（Ctrl+V 粘贴到播放头处）",
        "Copied {} (Ctrl+V pastes at the playhead)" => "已复制 {}（Ctrl+V 粘贴到播放头处）",
        "Pasted {}" => "已粘贴 {}",
        "Some notes landed past the end of the clip and won't play: the next clip on this track starts there" => {
            "有些音符落在了片段结尾之后，不会发声：这条音轨的下一个片段就从那里开始"
        }
        "Moved {} onto the grid" => "已把 {} 对齐到网格",
        "Zoom" => "缩放",
        "Height" => "行高",
        "Kick" => "底鼓",
        "Snare" => "军鼓",
        "Clap" => "拍手",
        "C.Hat" => "闭镲",
        "O.Hat" => "开镲",
        "Tom L" => "低通鼓",
        "Tom M" => "中通鼓",
        "Tom H" => "高通鼓",
        "Crash" => "吊镲",

        // ---- keys / scales ----
        "Any notes" => "任意音",
        "Major (happy)" => "大调（开心）",
        "Minor (sad)" => "小调（伤感）",
        "Pentatonic (can't go wrong)" => "五声音阶（怎么弹都对）",
        "Minor pentatonic" => "小调五声音阶",
        "Blues" => "布鲁斯",

        // ---- sound window ----
        "Playing through: {}" => "正在通过以下设备播放：{}",
        "⚠ The sound device is not running (it never asked for audio). Try another device below." => {
            "⚠ 声音设备没有运行（它从未请求音频数据）。请在下面换一个设备试试。"
        }
        "✔ Sound device is running ({} buffers so far)" => {
            "✔ 声音设备正在运行（已处理 {} 个缓冲区）"
        }
        "Device error: {}" => "设备错误：{}",
        "🔔 Play a test beep" => "🔔 播放测试音",
        "Meter moves but you hear nothing? Check the Windows volume (speaker icon in the taskbar), that the right speakers/headphones are the default, and that they aren't muted." => {
            "电平表在动但听不到声音？检查一下 Windows 音量（任务栏的扬声器图标），确认正确的扬声器/耳机是默认设备，并且没有静音。"
        }
        "🔇 AIDAW couldn't open any sound device." => "🔇 AIDAW 打不开任何声音设备。",
        "Choose speakers / headphones:" => "选择扬声器 / 耳机：",
        "{}  (Windows default)" => "{}  （Windows 默认）",
        "No output devices found. Plug in speakers or headphones." => {
            "没有找到输出设备。请插上扬声器或耳机。"
        }
        "Refresh list" => "刷新列表",

        // ---- tutorial ----
        "Welcome! Let's make some noise 🎉" => "欢迎！我们来弄出点声音吧 🎉",
        "This is a short song an AI wrote. In this tutorial you'll play it, change it, and make it yours.\n\nIt takes about 2 minutes. Click Next to begin." => {
            "这是 AI 写的一首小曲。在这个教程里，你会播放它、修改它，把它变成你自己的作品。\n\n大约需要 2 分钟。点击「下一步」开始。"
        }
        "1. Press Play" => "1. 按下播放",
        "Click the big ▶ Play button at the top (or press the Space bar) to hear the song." => {
            "点击顶部那个大大的 ▶ 播放 按钮（或按空格键）来听这首歌。"
        }
        "2. Pause it" => "2. 暂停",
        "Nice! Now click the button again (or press Space) to pause.\n\nTip: click on the numbers above the colored blocks to jump to any point in the song." => {
            "不错！再点一次这个按钮（或按空格）来暂停。\n\n小提示：点击彩色方块上方的数字，可以跳到歌曲的任意位置。"
        }
        "3. Open a clip" => "3. 打开一个片段",
        "The colored blocks are clips: short pieces of music. Each row is one instrument (a track).\n\nClick the green 'Melody' clip to open it." => {
            "彩色方块叫做片段：一小段一小段的音乐。每一行是一种乐器（一条音轨）。\n\n点击绿色的「Melody」片段把它打开。"
        }
        "4. Draw a note" => "4. 画一个音符",
        "The bottom area is the piano roll. Each row is a piano key (low at the bottom, high at the top) and left-to-right is time.\n\nClick an empty spot in the grid to add a note. You'll hear it!" => {
            "下方区域是钢琴卷帘。每一行是一个琴键（下面低、上面高），从左到右是时间。\n\n在网格的空白处点一下就能添加音符，你会听到它！"
        }
        "5. Move a note" => "5. 移动音符",
        "Drag a note up or down to change how high it sounds, or left and right to change when it plays.\n\nDrag the right edge of a note to make it longer." => {
            "上下拖动音符可以改变音高，左右拖动可以改变它什么时候响。\n\n拖动音符的右边缘可以让它变长。"
        }
        "6. Delete a note" => "6. 删除音符",
        "Right-click a note to delete it." => "右键点击音符就能删除它。",
        "7. Made a mistake? Undo!" => "7. 弄错了？撤销！",
        "Press Ctrl+Z to undo the last thing you did (Ctrl+Y to redo). You can't break anything here, so experiment freely.\n\nTry Ctrl+Z now." => {
            "按 Ctrl+Z 撤销刚才的操作（Ctrl+Y 重做）。在这里你什么都弄不坏，尽管大胆尝试。\n\n现在试试按 Ctrl+Z。"
        }
        "8. Change the sound" => "8. 换个音色",
        "On the left, under 'Sound', pick a different sound for the selected track, for example '🔔 Bell'. You'll hear it right away." => {
            "在左边的「音色」下面，给选中的音轨换一个音色，比如「🔔 铃声」。马上就能听到效果。"
        }
        "9. Make a new clip" => "9. 新建片段",
        "Double-click an empty spot in any track row to create a new, empty clip. Then draw notes in it like before." => {
            "在任意音轨行的空白处双击，就能新建一个空片段。然后像刚才一样在里面画音符。"
        }
        "10. Stay in key 🎵" => "10. 保持在调内 🎵",
        "Above the notes, the 🎵 row lights up the notes that sound good together, and every note you draw lands on one of them automatically.\n\nIn 🐣 Easy mode pick a Mood (Happy, Sad…). In 🔧 Pro mode you choose any key and can turn 'Stay in key' off." => {
            "在音符上方，🎵 那一行会把搭配起来好听的音点亮，你画的每个音符都会自动落在这些音上。\n\n在 🐣 简单模式下选一种情绪（开心、伤感……）。在 🔧 专业模式下可以选任意调，也可以关掉「保持在调内」。"
        }
        "You did it! 🏆" => "你做到了！🏆",
        "That's all you need to make music here.\n\nThe 🐣 Easy | 🔧 Pro switch at the top left shows more or fewer controls whenever you like. Your song never changes when you flip it.\n\nWant an AI to write music with you? Click ✨ Ask AI at the top and follow the steps. Whatever the AI writes shows up here, and you can change it just like you did now.\n\nYou can replay this tutorial any time from Help." => {
            "在这里做音乐需要的就是这些了。\n\n左上角的 🐣 简单 | 🔧 专业 开关可以随时显示更多或更少的控件，切换时歌曲不会变。\n\n想让 AI 和你一起写歌？点击顶部的 ✨ 问 AI，按步骤操作就行。AI 写的东西都会出现在这里，你可以像刚才那样随意修改。\n\n随时可以在「帮助」里重新打开这个教程。"
        }
        "Tutorial: demo song loaded" => "教程：示例歌曲已载入",
        "🎓 Tutorial" => "🎓 教程",
        "Step {} of {}" => "第 {} 步，共 {} 步",
        "✔ Nice!" => "✔ 很棒！",
        "Back" => "上一步",
        "Finish 🎉" => "完成 🎉",
        "Next ▶" => "下一步 ▶",
        "Skip this step ▶" => "跳过这一步 ▶",
        "Tutorial finished. Have fun making music!" => "教程完成，尽情玩音乐吧！",

        // ---- welcome screen ----
        "Welcome to AIDAW 👋" => "欢迎使用 AIDAW 👋",
        "Make music with an AI partner. No experience needed." => {
            "和 AI 搭档一起做音乐，不需要任何经验。"
        }
        "🎓  Show me how (2-minute tutorial)" => "🎓  教我怎么用（2 分钟教程）",
        "Best if this is your first time" => "第一次使用的话推荐这个",
        "🎵  Play the demo song" => "🎵  播放示例歌曲",
        "Hear what an AI made, then change it" => "听听 AI 做的歌，然后动手改一改",
        "✨  Start a new song" => "✨  新建一首歌",
        "An empty song with piano and drums" => "一首只带钢琴和鼓的空白歌曲",
        "💬  Let an AI make music for me" => "💬  让 AI 帮我作曲",
        "Connect Claude in 3 steps" => "3 步连接 Claude",
        "You can switch between 🐣 Easy and 🔧 Pro any time (top left)." => {
            "随时可以在左上角切换 🐣 简单 和 🔧 专业 模式。"
        }
        "Show this when AIDAW starts" => "启动 AIDAW 时显示这个界面",
        "Close" => "关闭",

        // ---- AI guide ----
        "💬 Let AI make music" => "💬 让 AI 来作曲",
        "AIDAW lets an AI assistant write music straight into your song. You watch it appear here and can change anything it makes. Claude is the easiest to set up (below); other AI apps work too." => {
            "AIDAW 能让 AI 助手直接往你的歌里写音乐。你会看到它出现在这里，并且可以随意修改 AI 写的任何内容。Claude 设置起来最简单（见下方），其他 AI 应用也可以用。"
        }
        "Step 1: Save your song" => "第 1 步：保存歌曲",
        "✔ Saved as {}" => "✔ 已保存为 {}",
        "The AI works on a song file, so save it first." => "AI 是在歌曲文件上工作的，所以请先保存。",
        "💾 Save song…" => "💾 保存歌曲…",
        "Step 2: Tell Claude Desktop about AIDAW" => "第 2 步：让 Claude Desktop 认识 AIDAW",
        "Copy this, then in Claude Desktop open Settings › Developer › Edit Config, paste it into claude_desktop_config.json, save, and restart Claude Desktop." => {
            "复制下面的内容，然后在 Claude Desktop 里打开 设置 › 开发者 › 编辑配置，把它粘贴进 claude_desktop_config.json，保存后重启 Claude Desktop。"
        }
        "Copy" => "复制",
        "Open Claude config folder" => "打开 Claude 配置文件夹",
        "Already have other servers in that file? Add just the \"aidaw\": {…} part inside your existing \"mcpServers\"." => {
            "那个文件里已经有其他服务器了？只需把 \"aidaw\": {…} 这一段加进现有的 \"mcpServers\" 里。"
        }
        "Step 3: Ask for music" => "第 3 步：开口要音乐",
        "Keep this window open and ask Claude something like:" => "保持这个窗口打开，然后对 Claude 说类似这样的话：",
        "“Make a chill lo-fi beat at 85 BPM with piano chords, bass and drums.”" => {
            "“做一段 85 BPM 的轻松 lo-fi 节拍，要有钢琴和弦、贝斯和鼓。”"
        }
        "“Add a happy melody on a new track over bars 5 to 8.”" => {
            "“在新音轨的第 5 到 8 小节加一段欢快的旋律。”"
        }
        "“The drums are too busy. Make them simpler.”" => "“鼓太密了，弄简单一点。”",
        "Don't like what it did? Press Ctrl+Z here to undo it." => {
            "不喜欢它改的？在这里按 Ctrl+Z 撤销就行。"
        }
        "Using a different AI (Cursor, ChatGPT, Gemini, Ollama, …)" => {
            "使用其他 AI（Cursor、ChatGPT、Gemini、Ollama……）"
        }
        "Any app that supports MCP servers (Cursor, Windsurf, Cline, Claude Code…) uses the same aidaw-mcp program as above." => {
            "任何支持 MCP 服务器的应用（Cursor、Windsurf、Cline、Claude Code……）都使用上面同一个 aidaw-mcp 程序。"
        }
        "For other AIs, aidaw-mcp also works as a plain command. An AI that can run commands can use it directly, for example:" => {
            "对于其他 AI，aidaw-mcp 也可以当普通命令行程序用。能运行命令的 AI 可以直接调用它，例如："
        }
        "'guide' prints a cheat sheet to paste into any chat AI. 'tools --format openai' prints function definitions for ChatGPT-style APIs. Full instructions: LLM_GUIDE.md next to aidaw.exe." => {
            "'guide' 会输出一份速查表，可以粘贴给任何聊天 AI。'tools --format openai' 会输出 ChatGPT 风格 API 用的函数定义。完整说明见 aidaw.exe 旁边的 LLM_GUIDE.md。"
        }
        "(save your song first)" => "（请先保存歌曲）",
        "No setup at all: paste notes from any chat AI" => "什么都不想装？从任何聊天 AI 粘贴音符",
        "Too lazy to set anything up? Fine. Give any chat AI (ChatGPT, Gemini, DeepSeek…) this request, copy its whole answer, click a clip here and press Ctrl+V. The notes land in the bar where the playhead is, and Ctrl+Z takes them back if the AI wrote rubbish." => {
            "懒得配置？哼，真拿你没办法～ 把下面这段话发给随便哪个聊天 AI（ChatGPT、Gemini、DeepSeek……），复制它的整段回答，在这里点一个片段，按 Ctrl+V 就好。音符会落在播放头所在的小节里；AI 写得像垃圾的话，Ctrl+Z 一下就没了，杂鱼也搞不砸的啦。"
        }
        "Write a 4-bar melody in A minor. Answer with a JSON list of notes like [{\"pitch\": \"A4\", \"start\": 0, \"duration\": 1, \"velocity\": 100}]. start and duration are in beats (4 beats = 1 bar); pitch is a note name or MIDI number." => {
            "用 A 小调写一段 4 小节的旋律。用 JSON 音符列表回答，格式像 [{\"pitch\": \"A4\", \"start\": 0, \"duration\": 1, \"velocity\": 100}]。start 和 duration 以拍为单位（4 拍 = 1 小节）；pitch 用音名或 MIDI 编号。"
        }
        "Copied! Paste it into the chat, then copy the AI's answer and press Ctrl+V in a clip." => {
            "复制好了！粘贴到聊天里，再把 AI 的回答复制回来，在片段里按 Ctrl+V。"
        }

        // ---- shortcuts ----
        "Space" => "空格",
        "Play / pause" => "播放 / 暂停",
        "Undo / redo" => "撤销 / 重做",
        "Save" => "保存",
        "Click the bar numbers" => "点击小节号",
        "Jump to that point in the song" => "跳到歌曲的那个位置",
        "Double-click a track row" => "双击音轨行",
        "New clip" => "新建片段",
        "Drag a clip / its right edge" => "拖动片段 / 片段右边缘",
        "Move / make longer" => "移动 / 加长",
        "Click the piano roll grid" => "点击钢琴卷帘网格",
        "Add a note" => "添加音符",
        "Drag a note / its right edge" => "拖动音符 / 音符右边缘",
        "Right-click a note" => "右键点击音符",
        "Delete it" => "删除它",
        "Hold or slide on piano keys" => "在琴键上按住或滑动",
        "Hear notes" => "试听音符",
        "Delete" => "Delete 键",
        "Delete the selected notes, or the clip you clicked last" => "删除选中的音符，或最后点过的片段",
        "Shift+drag on empty grid" => "在空白网格上按住 Shift 拖动",
        "Select several notes (Ctrl+drag adds more)" => "框选多个音符（按住 Ctrl 拖动可以追加）",
        "Ctrl+click a note" => "按住 Ctrl 点击音符",
        "Add it to the selection or take it out" => "把它加入选择，或移出选择",
        "Select every note in the clip" => "选中片段里的所有音符",
        "Drag a selected note" => "拖动一个选中的音符",
        "Move all selected notes together" => "所有选中的音符一起移动",
        "Copy / cut them" => "复制 / 剪切它们",
        "Paste at the playhead (notes copied from an AI chat work too)" => {
            "粘贴到播放头处（从 AI 聊天里复制的音符也行）"
        }
        "Duplicate: repeat them right after themselves" => "重复：紧接着在后面再来一份",
        "Left / Right arrow" => "左 / 右方向键",
        "Move selected notes by the grid (Shift: by a bar)" => "选中的音符左右移动一格（Shift：一个小节）",
        "Up / Down arrow" => "上 / 下方向键",
        "Move selected notes up or down, staying in key (Shift: an octave)" => {
            "选中的音符升高或降低，保持在调内（Shift：一个八度）"
        }
        "Snap the selected notes (or all) to the grid" => "把选中的音符（或全部）对齐到网格",
        "Select nothing" => "取消选择",
        "Ctrl+mouse wheel" => "Ctrl + 鼠标滚轮",
        "Zoom the piano roll" => "缩放钢琴卷帘",

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_fill_in_order() {
        set(Lang::En);
        assert_eq!(trf("Step {} of {}", &[&2, &12]), "Step 2 of 12");
        set(Lang::Zh);
        assert_eq!(trf("Step {} of {}", &[&2, &12]), "第 2 步，共 12 步");
        assert_eq!(tr("not in the table"), "not in the table");
        set(Lang::En);
    }

    /// Read the string literal that starts right after an opening quote.
    fn literal(s: &str) -> Option<String> {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    other => out.push(other),
                },
                c => out.push(c),
            }
        }
        None
    }

    /// Every `tr("…")` / `trf("…")` in the GUI, plus the tutorial and preset
    /// text that is translated where it is shown, has a Chinese entry.
    #[test]
    fn every_string_is_translated() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut missing = Vec::new();
        let mut texts = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let src = std::fs::read_to_string(&path).unwrap();
            let src = src.split("#[cfg(test)]").next().unwrap();
            for pat in ["tr(", "trf(", "title: ", "body: "] {
                for (i, _) in src.match_indices(pat) {
                    let before = src[..i].chars().next_back();
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    // The text may start on the next line: `tr(\n    "…")`.
                    let rest = src[i + pat.len()..].trim_start();
                    if let Some(lit) = rest.strip_prefix('"').and_then(literal) {
                        texts.push(lit);
                    }
                }
            }
        }
        for w in aidaw_core::Waveform::ALL {
            texts.push(w.label().to_string());
        }
        for p in aidaw_core::presets::all() {
            texts.push(p.name.to_string());
            texts.push(p.hint.to_string());
        }
        for t in &texts {
            if zh(t).is_none() {
                missing.push(t.clone());
            }
        }
        assert!(texts.len() > 200, "found only {} strings", texts.len());
        assert!(missing.is_empty(), "no Chinese for: {missing:#?}");
    }
}
