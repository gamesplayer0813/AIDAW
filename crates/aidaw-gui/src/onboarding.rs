//! Everything for people who have never used a DAW: the welcome screen, the
//! step-by-step interactive tutorial, the "let AI make music" guide and the
//! shortcut cheat-sheet.

use std::time::{Duration, Instant};

use aidaw_core::{Instrument, Project};
use eframe::egui::{self, Align2, Color32, Id, LayerId, Order, RichText, Stroke, StrokeKind};

use crate::app::App;
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};

pub const DEMO_SONG: &str = include_str!("../../../examples/demo.aidaw.json");

pub fn demo_project() -> Project {
    serde_json::from_str(DEMO_SONG).expect("embedded demo song is valid")
}

// ---- tutorial --------------------------------------------------------------

/// Values captured when a step starts, so a step can check "did something change?".
struct Base {
    notes: usize,
    clips: usize,
    instruments: Vec<Instrument>,
    note_drags: u32,
    undos: u32,
}

impl Base {
    fn capture(app: &App) -> Base {
        Base {
            notes: count_notes(&app.project),
            clips: app.project.tracks.iter().map(|t| t.clips.len()).sum(),
            instruments: app
                .project
                .tracks
                .iter()
                .map(|t| t.instrument.clone())
                .collect(),
            note_drags: app.ev_note_drags,
            undos: app.ev_undos,
        }
    }
}

fn count_notes(p: &Project) -> usize {
    p.tracks
        .iter()
        .flat_map(|t| &t.clips)
        .map(|c| c.notes.len())
        .sum()
}

struct Step {
    title: &'static str,
    body: &'static str,
    /// Key into `App::ui_rects` of the area to highlight.
    target: &'static str,
    check: fn(&App, &Base) -> bool,
}

const STEPS: &[Step] = &[
    Step {
        title: "Welcome! Let's make some noise 🎉",
        body: "This is a short song an AI wrote. In this tutorial you'll play it, change it, and make it yours.\n\nIt takes about 2 minutes. Click Next to begin.",
        target: "",
        check: |_, _| false,
    },
    Step {
        title: "1. Press Play",
        body: "Click the big ▶ Play button at the top (or press the Space bar) to hear the song.",
        target: "play",
        check: |app, _| app.is_playing(),
    },
    Step {
        title: "2. Pause it",
        body: "Nice! Now click the button again (or press Space) to pause.\n\nTip: click on the numbers above the colored blocks to jump to any point in the song.",
        target: "play",
        check: |app, _| !app.is_playing(),
    },
    Step {
        title: "3. Open a clip",
        body: "The colored blocks are clips: short pieces of music. Each row is one instrument (a track).\n\nClick the green 'Melody' clip to open it.",
        target: "arrange",
        check: |app, _| {
            app.selected_clip
                .as_deref()
                .and_then(|id| app.project.clip(id))
                .is_some_and(|(t, _)| !t.instrument.is_drums())
        },
    },
    Step {
        title: "4. Draw a note",
        body: "The bottom area is the piano roll. Each row is a piano key (low at the bottom, high at the top) and left-to-right is time.\n\nClick an empty spot in the grid to add a note. You'll hear it!",
        target: "piano",
        check: |app, b| count_notes(&app.project) > b.notes,
    },
    Step {
        title: "5. Move a note",
        body: "Drag a note up or down to change how high it sounds, or left and right to change when it plays.\n\nDrag the right edge of a note to make it longer.",
        target: "piano",
        check: |app, b| app.ev_note_drags > b.note_drags,
    },
    Step {
        title: "6. Delete a note",
        body: "Right-click a note to delete it.",
        target: "piano",
        check: |app, b| count_notes(&app.project) < b.notes,
    },
    Step {
        title: "7. Made a mistake? Undo!",
        body: "Press Ctrl+Z to undo the last thing you did (Ctrl+Y to redo). You can't break anything here, so experiment freely.\n\nTry Ctrl+Z now.",
        target: "",
        check: |app, b| app.ev_undos > b.undos,
    },
    Step {
        title: "8. Change the sound",
        body: "On the left, under 'Sound', pick a different sound for the selected track, for example '🔔 Bell'. You'll hear it right away.",
        target: "sound",
        check: |app, b| app.project.tracks.iter().map(|t| &t.instrument).ne(b.instruments.iter()),
    },
    Step {
        title: "9. Make a new clip",
        body: "Double-click an empty spot in any track row to create a new, empty clip. Then draw notes in it like before.",
        target: "arrange",
        check: |app, b| app.project.tracks.iter().map(|t| t.clips.len()).sum::<usize>() > b.clips,
    },
    Step {
        title: "10. Stay in key 🎵",
        body: "Above the notes, the 🎵 row lights up the notes that sound good together, and every note you draw lands on one of them automatically.\n\nIn 🐣 Easy mode pick a Mood (Happy, Sad…). In 🔧 Pro mode you choose any key and can turn 'Stay in key' off.",
        target: "key",
        check: |_, _| false,
    },
    Step {
        title: "You did it! 🏆",
        body: "That's all you need to make music here.\n\nThe 🐣 Easy | 🔧 Pro switch at the top left shows more or fewer controls whenever you like. Your song never changes when you flip it.\n\nWant an AI to write music with you? Click ✨ Ask AI at the top and follow the steps. Whatever the AI writes shows up here, and you can change it just like you did now.\n\nYou can replay this tutorial any time from Help.",
        target: "mode",
        check: |_, _| false,
    },
];

pub struct Tutorial {
    step: usize,
    base: Base,
    done_at: Option<Instant>,
}

impl Tutorial {
    pub fn start(app: &mut App) -> Tutorial {
        app.load_project(demo_project(), None, tr("Tutorial: demo song loaded"));
        app.selected_clip = None;
        app.selected_note = None;
        Tutorial {
            step: 0,
            base: Base::capture(app),
            done_at: None,
        }
    }

    fn go(&mut self, app: &App, step: usize) {
        self.step = step.min(STEPS.len() - 1);
        self.base = Base::capture(app);
        self.done_at = None;
    }
}

/// Draw the tutorial window and highlight. Returns false when it should close.
pub fn show_tutorial(ctx: &egui::Context, app: &mut App, tut: &mut Tutorial) -> bool {
    let step = &STEPS[tut.step];
    if tut.done_at.is_none() && (step.check)(app, &tut.base) {
        tut.done_at = Some(Instant::now());
    }
    // Auto-advance a moment after the step is completed so the ✔ is visible.
    if tut
        .done_at
        .is_some_and(|t| t.elapsed() > Duration::from_millis(1200))
    {
        tut.go(app, tut.step + 1);
    }
    let step = &STEPS[tut.step];
    let done = tut.done_at.is_some();

    if let Some(rect) = app.ui_rects.get(step.target) {
        let t = ctx.input(|i| i.time) as f32;
        let alpha = 0.55 + 0.45 * (t * 4.0).sin();
        let painter = ctx.layer_painter(LayerId::new(
            Order::Foreground,
            Id::new("tutorial_highlight"),
        ));
        let color = Color32::from_rgb(255, 210, 60).gamma_multiply(alpha);
        painter.rect_stroke(
            rect.expand(3.0),
            6.0,
            Stroke::new(3.0_f32, color),
            StrokeKind::Outside,
        );
        ctx.request_repaint();
    }

    let mut open = true;
    let mut finish = false;
    egui::Window::new(RichText::new(tr("🎓 Tutorial")).strong())
        .id(Id::new("tutorial_window"))
        // Keep the window away from whatever the step asks you to click.
        .anchor(
            if step.target == "piano" || step.target == "key" {
                Align2::RIGHT_TOP
            } else {
                Align2::RIGHT_BOTTOM
            },
            if step.target == "piano" || step.target == "key" {
                [-16.0, 64.0]
            } else {
                [-16.0, -36.0]
            },
        )
        .collapsible(true)
        .resizable(false)
        .default_width(330.0)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_max_width(330.0);
            ui.label(RichText::new(trf("Step {} of {}", &[&(tut.step + 1), &STEPS.len()])).weak());
            ui.add(
                egui::ProgressBar::new((tut.step + 1) as f32 / STEPS.len() as f32)
                    .desired_height(4.0),
            );
            ui.add_space(4.0);
            ui.label(RichText::new(tr(step.title)).heading());
            ui.add_space(4.0);
            ui.label(tr(step.body));
            ui.add_space(8.0);
            if done {
                ui.label(
                    RichText::new(tr("✔ Nice!"))
                        .color(Color32::LIGHT_GREEN)
                        .strong()
                        .size(18.0),
                );
            }
            ui.horizontal(|ui| {
                if tut.step > 0
                    && ui
                        .add(icons::button(Icon::ChevronLeft, tr("Back")))
                        .clicked()
                {
                    tut.go(app, tut.step - 1);
                }
                let last = tut.step + 1 == STEPS.len();
                let next_label = if last {
                    tr("Finish 🎉")
                } else if tut.step == 0 || matches!(step.target, "key") {
                    tr("Next ▶")
                } else {
                    tr("Skip this step ▶")
                };
                let next = egui::Button::new(RichText::new(next_label).strong());
                if ui.add(next).clicked() {
                    if last {
                        finish = true;
                    } else {
                        tut.go(app, tut.step + 1);
                    }
                }
            });
        });
    if finish {
        app.settings.tutorial_done = true;
        app.settings.save();
        app.status = tr("Tutorial finished. Have fun making music!").into();
        return false;
    }
    open
}

// ---- welcome screen ----------------------------------------------------------

pub enum WelcomeChoice {
    None,
    Demo,
    Tutorial,
    Blank,
    AiHelp,
    Close,
}

pub fn show_welcome(ctx: &egui::Context, app: &mut App) -> WelcomeChoice {
    let mut choice = WelcomeChoice::None;
    egui::Modal::new(Id::new("welcome")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            crate::mode::lang_switch(ui, app);
        });
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(tr("Welcome to AIDAW 👋")).size(28.0).strong());
            ui.label(
                RichText::new(tr("Make music with an AI partner. No experience needed."))
                    .size(15.0),
            );
            ui.add_space(16.0);
            let big = |text: &str, sub: &str| {
                egui::Button::new(RichText::new(format!("{text}\n{sub}")).size(16.0))
                    .min_size(egui::vec2(400.0, 56.0))
            };
            if ui
                .add(big(
                    tr("🎓  Show me how (2-minute tutorial)"),
                    tr("Best if this is your first time"),
                ))
                .clicked()
            {
                choice = WelcomeChoice::Tutorial;
            }
            ui.add_space(6.0);
            if ui
                .add(big(
                    tr("🎵  Play the demo song"),
                    tr("Hear what an AI made, then change it"),
                ))
                .clicked()
            {
                choice = WelcomeChoice::Demo;
            }
            ui.add_space(6.0);
            if ui
                .add(big(
                    tr("✨  Start a new song"),
                    tr("An empty song with piano and drums"),
                ))
                .clicked()
            {
                choice = WelcomeChoice::Blank;
            }
            ui.add_space(6.0);
            if ui
                .add(big(
                    tr("💬  Let an AI make music for me"),
                    tr("Connect Claude in 3 steps"),
                ))
                .clicked()
            {
                choice = WelcomeChoice::AiHelp;
            }
            ui.add_space(10.0);
            ui.label(
                RichText::new(tr(
                    "You can switch between 🐣 Easy and 🔧 Pro any time (top left).",
                ))
                .weak(),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .checkbox(
                        &mut app.settings.show_welcome,
                        tr("Show this when AIDAW starts"),
                    )
                    .changed()
                {
                    app.settings.save();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Close")).clicked() {
                        choice = WelcomeChoice::Close;
                    }
                });
            });
        });
    });
    choice
}

// ---- AI guide ------------------------------------------------------------------

pub fn show_ai_help(ctx: &egui::Context, app: &mut App, open: &mut bool) {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let mcp_exe = exe_dir.as_ref().map(|d| {
        d.join(if cfg!(windows) {
            "aidaw-mcp.exe"
        } else {
            "aidaw-mcp"
        })
    });
    let mut save_clicked = false;
    egui::Window::new(tr("💬 Let AI make music"))
        .id(Id::new("ai_help_window"))
        .open(open)
        .collapsible(false)
        .default_width(520.0)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(tr("AIDAW lets an AI assistant write music straight into your song. You watch it appear here and can change anything it makes. Claude is the easiest to set up (below); other AI apps work too."));
            ui.add_space(8.0);

            ui.label(RichText::new(tr("Step 1: Save your song")).strong());
            match &app.path {
                Some(p) => {
                    ui.label(RichText::new(trf("✔ Saved as {}", &[&p.display()])).color(Color32::LIGHT_GREEN));
                }
                None => {
                    ui.label(tr("The AI works on a song file, so save it first."));
                    if ui.button(tr("💾 Save song…")).clicked() {
                        save_clicked = true;
                    }
                }
            }
            ui.add_space(8.0);

            ui.label(RichText::new(tr("Step 2: Tell Claude Desktop about AIDAW")).strong());
            ui.label(tr("Copy this, then in Claude Desktop open Settings › Developer › Edit Config, paste it into claude_desktop_config.json, save, and restart Claude Desktop."));
            let config = config_snippet(mcp_exe.as_deref(), app.path.as_deref());
            ui.add(
                egui::TextEdit::multiline(&mut config.as_str())
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(8)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if ui.add(icons::button(Icon::Clipboard, tr("Copy"))).clicked() {
                    ui.ctx().copy_text(config.clone());
                    app.status = tr("Copied! Paste it into Claude Desktop's config file.").into();
                }
                if cfg!(windows)
                    && ui.add(icons::button(Icon::Folder, tr("Open Claude config folder"))).clicked() {
                    if let Some(appdata) = std::env::var_os("APPDATA") {
                        let dir = std::path::PathBuf::from(appdata).join("Claude");
                        let _ = std::process::Command::new("explorer").arg(dir).spawn();
                    }
                }
            });
            ui.weak(tr("Already have other servers in that file? Add just the \"aidaw\": {…} part inside your existing \"mcpServers\"."));
            ui.add_space(8.0);

            ui.label(RichText::new(tr("Step 3: Ask for music")).strong());
            ui.label(tr("Keep this window open and ask Claude something like:"));
            for example in [
                tr("“Make a chill lo-fi beat at 85 BPM with piano chords, bass and drums.”"),
                tr("“Add a happy melody on a new track over bars 5 to 8.”"),
                tr("“The drums are too busy. Make them simpler.”"),
            ] {
                ui.label(RichText::new(example).italics());
            }
            ui.add_space(4.0);
            ui.label(tr("Don't like what it did? Press Ctrl+Z here to undo it."));
            ui.add_space(8.0);
            egui::CollapsingHeader::new(tr("Using a different AI (Cursor, ChatGPT, Gemini, Ollama, …)"))
                .id_salt("other_ai")
                .show(ui, |ui| {
                    ui.label(tr("Any app that supports MCP servers (Cursor, Windsurf, Cline, Claude Code…) uses the same aidaw-mcp program as above."));
                    ui.label(tr("For other AIs, aidaw-mcp also works as a plain command. An AI that can run commands can use it directly, for example:"));
                    let exe = mcp_exe
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "aidaw-mcp".into());
                    let project = app
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "song.aidaw.json".into());
                    let cmd = format!(
                        "\"{exe}\" call add_chords --project \"{project}\" track=Piano chords=\"Am F C G\"\n\"{exe}\" guide\n\"{exe}\" tools --format openai"
                    );
                    ui.add(
                        egui::TextEdit::multiline(&mut cmd.as_str())
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    if ui.add(icons::button(Icon::Clipboard, tr("Copy"))).clicked() {
                        ui.ctx().copy_text(cmd.clone());
                    }
                    ui.weak(tr("'guide' prints a cheat sheet to paste into any chat AI. 'tools --format openai' prints function definitions for ChatGPT-style APIs. Full instructions: LLM_GUIDE.md next to aidaw.exe."));
                });
        });
    if save_clicked {
        app.save_as_dialog();
    }
}

fn config_snippet(mcp_exe: Option<&std::path::Path>, project: Option<&std::path::Path>) -> String {
    let exe = mcp_exe
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "C:\\path\\to\\aidaw-mcp.exe".into());
    let project = project
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| tr("(save your song first)").into());
    // Written by hand (not via serde_json) to keep "command" before "args".
    let q = |v: &str| serde_json::to_string(v).unwrap_or_default();
    format!(
        "{{\n  \"mcpServers\": {{\n    \"aidaw\": {{\n      \"command\": {},\n      \"args\": [\"--project\", {}]\n    }}\n  }}\n}}",
        q(&exe),
        q(&project)
    )
}

// ---- shortcuts -----------------------------------------------------------------

pub fn show_shortcuts(ctx: &egui::Context, open: &mut bool) {
    egui::Window::new(tr("⌨ Keyboard & mouse"))
        .id(Id::new("shortcuts_window"))
        .open(open)
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("shortcuts")
                .num_columns(2)
                .spacing([24.0, 6.0])
                .striped(true)
                .show(ui, |ui| {
                    for (keys, what) in [
                        (tr("Space"), tr("Play / pause")),
                        ("Ctrl+Z / Ctrl+Y", tr("Undo / redo")),
                        ("Ctrl+S", tr("Save")),
                        (
                            tr("Click the bar numbers"),
                            tr("Jump to that point in the song"),
                        ),
                        (tr("Double-click a track row"), tr("New clip")),
                        (tr("Drag a clip / its right edge"), tr("Move / make longer")),
                        (tr("Click the piano roll grid"), tr("Add a note")),
                        (tr("Drag a note / its right edge"), tr("Move / make longer")),
                        (tr("Right-click a note"), tr("Delete it")),
                        (tr("Hold or slide on piano keys"), tr("Hear notes")),
                        (tr("Delete"), tr("Delete the selected note or clip")),
                        (tr("Ctrl+mouse wheel"), tr("Zoom the piano roll")),
                    ] {
                        ui.label(RichText::new(keys).monospace().strong());
                        ui.label(what);
                        ui.end_row();
                    }
                });
        });
}
