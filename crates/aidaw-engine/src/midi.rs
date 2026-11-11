//! MIDI keyboards. Every MIDI input (or one chosen by name) is opened; its
//! notes play the live track's instrument at once, through the output
//! callback's live queue, and are reported with the song beat they were
//! played on so the app can record them.

use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;

use midir::{MidiInput, MidiInputConnection};

use crate::clock::Clock;

const CLIENT: &str = "AIDAW";

/// A key, or the sustain pedal, on a MIDI keyboard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MidiEvent {
    NoteOn {
        pitch: u8,
        velocity: u8,
        /// The song beat heard when the key went down; `None` while the
        /// song stands still.
        beat: Option<f64>,
    },
    NoteOff {
        pitch: u8,
        beat: Option<f64>,
    },
    Pedal {
        down: bool,
        beat: Option<f64>,
    },
}

/// What the output callback does with a MIDI message.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Live {
    On { pitch: u8, velocity: u8 },
    Off { pitch: u8 },
    Pedal(bool),
    AllOff,
}

/// Understand one MIDI message (any channel); `None` for the rest.
pub(crate) fn parse(msg: &[u8]) -> Option<Live> {
    let (&status, data) = msg.split_first()?;
    let d = |i: usize| data.get(i).map(|b| b & 0x7f);
    match status & 0xf0 {
        0x90 if d(1)? > 0 => Some(Live::On {
            pitch: d(0)?,
            velocity: d(1)?,
        }),
        // Note-on with velocity 0 is how many keyboards send note-off.
        0x80 | 0x90 => Some(Live::Off { pitch: d(0)? }),
        0xb0 => match d(0)? {
            64 => Some(Live::Pedal(d(1)? >= 64)),
            // All sound off, all notes off.
            120 | 123 => Some(Live::AllOff),
            _ => None,
        },
        _ => None,
    }
}

/// Names of the MIDI inputs on this computer.
pub fn midi_inputs() -> Vec<String> {
    let Ok(midi) = MidiInput::new(CLIENT) else {
        return Vec::new();
    };
    midi.ports()
        .iter()
        .filter_map(|p| midi.port_name(p).ok())
        .collect()
}

/// Why a MIDI keyboard can't be played.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MidiProblem {
    /// The system's MIDI service didn't start.
    Unavailable(String),
    /// A keyboard that is plugged in couldn't be opened (usually another
    /// program has it).
    Port { name: String, error: String },
    /// The keyboard asked for by name isn't plugged in.
    Missing(String),
}

impl std::fmt::Display for MidiProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MidiProblem::Unavailable(e) => write!(f, "MIDI is not available: {e}"),
            MidiProblem::Port { name, error } => write!(f, "couldn't open {name}: {error}"),
            MidiProblem::Missing(name) => write!(f, "MIDI input {name:?} not found"),
        }
    }
}

/// Open MIDI inputs and where their notes go.
pub(crate) struct Midi {
    connections: Vec<(String, MidiInputConnection<()>)>,
    events: Receiver<MidiEvent>,
    live: SyncSender<Live>,
    problems: Vec<MidiProblem>,
}

impl Midi {
    /// Open the input called `only`, or every input when `None`.
    pub(crate) fn open(only: Option<&str>, live: SyncSender<Live>, clock: Arc<Clock>) -> Midi {
        let (events_tx, events) = mpsc::channel();
        let mut connections = Vec::new();
        let mut problems = Vec::new();
        match MidiInput::new(CLIENT) {
            Err(e) => problems.push(MidiProblem::Unavailable(e.to_string())),
            Ok(probe) => {
                let ports = probe.ports();
                for port in &ports {
                    let Ok(name) = probe.port_name(port) else {
                        continue;
                    };
                    if only.is_some_and(|o| o != name) {
                        continue;
                    }
                    let input = match MidiInput::new(CLIENT) {
                        Ok(i) => i,
                        Err(e) => {
                            problems.push(MidiProblem::Port {
                                name,
                                error: e.to_string(),
                            });
                            continue;
                        }
                    };
                    let (live, events_tx, clock) = (live.clone(), events_tx.clone(), clock.clone());
                    let callback = move |_stamp: u64, msg: &[u8], _: &mut ()| {
                        let Some(event) = parse(msg) else {
                            return;
                        };
                        // Sound first, then tell the app.
                        let _ = live.try_send(event);
                        let beat = clock.beat_now();
                        let report = match event {
                            Live::On { pitch, velocity } => MidiEvent::NoteOn {
                                pitch,
                                velocity,
                                beat,
                            },
                            Live::Off { pitch } => MidiEvent::NoteOff { pitch, beat },
                            Live::Pedal(down) => MidiEvent::Pedal { down, beat },
                            Live::AllOff => MidiEvent::Pedal { down: false, beat },
                        };
                        let _ = events_tx.send(report);
                    };
                    match input.connect(port, "AIDAW input", callback, ()) {
                        Ok(c) => connections.push((name, c)),
                        // Usually another app has the keyboard open.
                        Err(e) => problems.push(MidiProblem::Port {
                            name,
                            error: e.to_string(),
                        }),
                    }
                }
                if let (Some(name), true) = (only, connections.is_empty() && problems.is_empty()) {
                    problems.push(MidiProblem::Missing(name.to_string()));
                }
            }
        }
        Midi {
            connections,
            events,
            live,
            problems,
        }
    }

    /// The inputs that are open.
    pub(crate) fn ports(&self) -> Vec<String> {
        self.connections.iter().map(|(n, _)| n.clone()).collect()
    }

    /// Inputs that could not be opened, and why.
    pub(crate) fn problems(&self) -> &[MidiProblem] {
        &self.problems
    }

    /// Everything played since the last call.
    pub(crate) fn drain(&self) -> Vec<MidiEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for Midi {
    fn drop(&mut self) {
        // Closing the keyboard must not leave its notes hanging.
        let _ = self.live.try_send(Live::AllOff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_messages_become_live_notes() {
        assert_eq!(
            parse(&[0x90, 60, 100]),
            Some(Live::On {
                pitch: 60,
                velocity: 100
            })
        );
        // Any channel.
        assert_eq!(
            parse(&[0x9f, 61, 1]),
            Some(Live::On {
                pitch: 61,
                velocity: 1
            })
        );
        assert_eq!(parse(&[0x90, 60, 0]), Some(Live::Off { pitch: 60 }));
        assert_eq!(parse(&[0x83, 60, 64]), Some(Live::Off { pitch: 60 }));
        assert_eq!(parse(&[0xb0, 64, 127]), Some(Live::Pedal(true)));
        assert_eq!(parse(&[0xb0, 64, 0]), Some(Live::Pedal(false)));
        assert_eq!(parse(&[0xb0, 123, 0]), Some(Live::AllOff));
        // Other controllers, pitch bend, clock, sysex and broken messages.
        assert_eq!(parse(&[0xb0, 7, 100]), None);
        assert_eq!(parse(&[0xe0, 0, 64]), None);
        assert_eq!(parse(&[0xf8]), None);
        assert_eq!(parse(&[0x90, 60]), None);
        assert_eq!(parse(&[]), None);
    }
}
