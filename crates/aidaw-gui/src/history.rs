//! Undo and redo. After every frame with input the song is compared with
//! the version last seen; a change starts an undo step from that version,
//! and a whole gesture (a drag, a slider) makes one step. Comparing is far
//! cheaper than copying the song, which only happens when it did change.

use std::collections::{BTreeSet, VecDeque};

use aidaw_core::Project;

/// How many steps undo goes back.
const LIMIT: usize = 200;

/// The clip open in the piano roll and the indexes of its selected notes.
pub type Selection = Option<(String, BTreeSet<usize>)>;

/// A version of the song, with the notes that were selected in it, so that
/// undo and redo bring the selection back too.
pub struct Version {
    pub project: Project,
    /// `None` when unknown, as for a version the AI wrote.
    pub selection: Selection,
}

impl Version {
    /// A version without a known selection.
    pub fn of(project: Project) -> Self {
        Version {
            project,
            selection: None,
        }
    }
}

pub struct History {
    undo: VecDeque<Version>,
    redo: Vec<Version>,
    /// The song as last seen, with its selection: what an edit made since
    /// then is undone back to.
    seen: Version,
    /// The version from before the gesture in progress.
    pending: Option<Version>,
}

impl History {
    pub fn new(project: &Project) -> Self {
        History {
            undo: VecDeque::new(),
            redo: Vec::new(),
            seen: Version::of(project.clone()),
            pending: None,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Look for an edit: whether `project` changed since it was last seen.
    /// If it did, the version before the change starts an undo step,
    /// unless the gesture in progress already started one. `selection`
    /// is only asked for then.
    pub fn notice(&mut self, project: &Project, selection: impl FnOnce() -> Selection) -> bool {
        if *project == self.seen.project {
            return false;
        }
        let now = Version {
            project: project.clone(),
            selection: selection(),
        };
        let before = std::mem::replace(&mut self.seen, now);
        self.pending.get_or_insert(before);
        true
    }

    /// Whether an edit gesture is in progress (see [`Self::end_gesture`]).
    pub fn in_gesture(&self) -> bool {
        self.pending.is_some()
    }

    /// Keep the selection stored with the version last seen up to date, so
    /// the next undo step brings back what was selected before the edit.
    pub fn select(&mut self, clip: Option<&str>, notes: &BTreeSet<usize>) {
        let same = match (&self.seen.selection, clip) {
            (Some((c, n)), Some(open)) => c == open && n == notes,
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.seen.selection = clip.map(|c| (c.to_string(), notes.clone()));
        }
    }

    /// End the gesture in progress: the version from before it becomes one
    /// undo step, unless `project` is back to it.
    pub fn end_gesture(&mut self, project: &Project) {
        if let Some(before) = self.pending.take() {
            if before.project != *project {
                self.push(before);
            }
        }
    }

    /// Add an undo step that did not come from an edit here, such as the
    /// song from before the AI's change was loaded. Forgets the redo steps.
    pub fn push(&mut self, version: Version) {
        self.push_undo(version);
        self.redo.clear();
    }

    fn push_undo(&mut self, version: Version) {
        self.undo.push_back(version);
        if self.undo.len() > LIMIT {
            self.undo.pop_front();
        }
    }

    /// The version to undo back to. Hand the song it replaces to
    /// [`Self::undone`].
    pub fn take_undo(&mut self) -> Option<Version> {
        self.undo.pop_back()
    }

    /// The version to redo. Hand the song it replaces to [`Self::redone`].
    pub fn take_redo(&mut self) -> Option<Version> {
        self.redo.pop()
    }

    pub fn undone(&mut self, replaced: Version) {
        self.redo.push(replaced);
    }

    pub fn redone(&mut self, replaced: Version) {
        self.push_undo(replaced);
    }

    /// The song was replaced as a whole (undo, redo, a reload): it is the
    /// version seen now, and no gesture is in progress.
    pub fn replaced(&mut self, project: &Project, selection: Selection) {
        self.seen = Version {
            project: project.clone(),
            selection,
        };
        self.pending = None;
    }

    /// The song was just changed in a way that is not an edit to undo, such
    /// as the AI's playhead request: make the same change to the version
    /// last seen and to the one from before the gesture in progress, so it
    /// starts no undo step.
    pub fn not_an_edit(&mut self, change: impl Fn(&mut Project)) {
        change(&mut self.seen.project);
        if let Some(before) = &mut self.pending {
            change(&mut before.project);
        }
    }

    /// Forget every step, for a song from another file.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(bpm: f64) -> Project {
        Project::new("x", bpm)
    }

    fn selected(clip: &str, notes: &[usize]) -> Selection {
        Some((clip.into(), notes.iter().copied().collect()))
    }

    #[test]
    fn a_whole_gesture_is_one_undo_step() {
        let mut h = History::new(&song(100.0));
        assert!(!h.notice(&song(100.0), || unreachable!("nothing changed")));
        // A slider dragged through 101, 102, 103 while the mouse is held.
        for bpm in [101.0, 102.0, 103.0] {
            assert!(h.notice(&song(bpm), || None));
        }
        assert!(
            h.in_gesture() && !h.can_undo(),
            "not before the mouse is released"
        );
        h.end_gesture(&song(103.0));
        assert!(!h.in_gesture());
        let step = h.take_undo().expect("one step");
        assert_eq!(step.project.bpm, 100.0, "back to before the gesture");
        assert!(h.take_undo().is_none());
    }

    #[test]
    fn a_gesture_that_ends_where_it_started_is_no_step() {
        let mut h = History::new(&song(100.0));
        h.notice(&song(110.0), || None);
        h.notice(&song(100.0), || None);
        h.end_gesture(&song(100.0));
        assert!(!h.can_undo());
        h.end_gesture(&song(100.0)); // nothing in progress
        assert!(!h.can_undo());
    }

    #[test]
    fn undo_and_redo_bring_the_selection_back() {
        let mut h = History::new(&song(100.0));
        h.select(Some("c1"), &[0, 2].into());
        h.notice(&song(120.0), || selected("c1", &[5]));
        h.end_gesture(&song(120.0));
        h.select(Some("c1"), &[5].into());

        let back = h.take_undo().unwrap();
        assert_eq!(
            (back.project.bpm, back.selection),
            (100.0, selected("c1", &[0, 2]))
        );
        h.undone(Version {
            project: song(120.0),
            selection: selected("c1", &[5]),
        });
        h.replaced(&song(100.0), selected("c1", &[0, 2]));
        assert!(h.can_redo());

        let again = h.take_redo().unwrap();
        assert_eq!(again.selection, selected("c1", &[5]));
        h.redone(Version {
            project: song(100.0),
            selection: selected("c1", &[0, 2]),
        });
        assert!(h.can_undo() && !h.can_redo());
    }

    #[test]
    fn a_new_edit_forgets_the_redo_steps() {
        let mut h = History::new(&song(100.0));
        h.notice(&song(110.0), || None);
        h.end_gesture(&song(110.0));
        let back = h.take_undo().unwrap();
        h.undone(Version::of(song(110.0)));
        h.replaced(&back.project, None);
        assert!(h.can_redo());
        h.notice(&song(90.0), || None);
        h.end_gesture(&song(90.0));
        assert!(!h.can_redo());
        assert_eq!(h.take_undo().unwrap().project.bpm, 100.0);
    }

    #[test]
    fn a_replaced_song_drops_the_gesture_in_progress() {
        let mut h = History::new(&song(100.0));
        h.notice(&song(101.0), || None);
        h.replaced(&song(140.0), None); // the AI's version arrived
        h.end_gesture(&song(140.0));
        assert!(!h.can_undo());
        assert!(!h.notice(&song(140.0), || None), "seen already");
    }

    #[test]
    fn a_change_that_is_not_an_edit_starts_no_step() {
        let cued = |bpm: f64| {
            let mut p = song(bpm);
            p.transport.cue = Some(aidaw_core::Cue {
                beat: 16.0,
                play: None,
                id: 7,
            });
            p
        };
        let mut h = History::new(&song(100.0));
        h.not_an_edit(|p| p.transport.cue = cued(0.0).transport.cue);
        assert!(!h.notice(&cued(100.0), || None), "only the cue changed");
        assert!(!h.can_undo());

        // During a gesture: the step it makes doesn't bring the old cue back,
        // and a gesture that changed nothing else is still no step.
        let mut h = History::new(&song(100.0));
        h.notice(&song(110.0), || None);
        h.not_an_edit(|p| p.transport.cue = cued(0.0).transport.cue);
        h.notice(&cued(100.0), || None);
        h.end_gesture(&cued(100.0));
        assert!(!h.can_undo());
    }

    #[test]
    fn only_the_last_steps_are_kept() {
        let mut h = History::new(&song(0.0));
        for bpm in 1..=LIMIT + 50 {
            h.notice(&song(bpm as f64), || None);
            h.end_gesture(&song(bpm as f64));
        }
        let mut count = 0;
        let mut oldest = None;
        while let Some(v) = h.take_undo() {
            count += 1;
            oldest = Some(v.project.bpm);
        }
        assert_eq!(count, LIMIT);
        assert_eq!(oldest, Some(50.0), "the first 50 steps were dropped");
        h.push(Version::of(song(1.0)));
        h.clear();
        assert!(!h.can_undo() && !h.can_redo());
    }
}
