//! Undo and redo, as two stacks of earlier and later versions.
//!
//! It knows nothing of scenes: it keeps whatever versions it is given, each
//! under the name of the step that followed it, so a menu can say "Undo Move
//! robin". The engine decides what a version holds and how to go back to one.

/// The most steps kept; older ones are forgotten.
const MOST_STEPS: usize = 200;

/// A version, and the name of the step that left it.
#[derive(Debug, Clone)]
struct Step<T> {
    label: String,
    version: T,
}

/// Versions before and after the one in use.
#[derive(Debug, Clone)]
pub(crate) struct History<T> {
    /// Oldest first.
    past: Vec<Step<T>>,
    /// Nearest last, so the next redo is popped.
    future: Vec<Step<T>>,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self {
            past: Vec::new(),
            future: Vec::new(),
        }
    }
}

impl<T: Clone + PartialEq> History<T> {
    /// Keep `version`, the one in use just before the step `label`. Anything
    /// that could be redone is gone: a new step starts a new future.
    pub(crate) fn remember(&mut self, label: &str, version: T) {
        self.future.clear();
        self.past.push(Step {
            label: label.to_owned(),
            version,
        });
        if self.past.len() > MOST_STEPS {
            self.past.remove(0);
        }
    }

    /// The version to go back to from `current`, which can be redone from
    /// then on. A step that changed nothing, such as a drag let go where it
    /// began, is passed over rather than undone to no effect.
    pub(crate) fn undo(&mut self, current: T) -> Option<T> {
        while let Some(step) = self.past.pop() {
            if step.version != current {
                self.future.push(Step {
                    label: step.label,
                    version: current,
                });
                return Some(step.version);
            }
        }
        None
    }

    /// The version to go forward to from `current`, which can be undone again.
    pub(crate) fn redo(&mut self, current: T) -> Option<T> {
        let step = self.future.pop()?;
        self.past.push(Step {
            label: step.label,
            version: current,
        });
        Some(step.version)
    }

    /// The steps that can be undone, oldest first.
    pub(crate) fn undoable(&self) -> Vec<String> {
        self.past.iter().map(|step| step.label.clone()).collect()
    }

    /// The steps that can be redone, next first.
    pub(crate) fn redoable(&self) -> Vec<String> {
        self.future
            .iter()
            .rev()
            .map(|step| step.label.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_walk_back_and_forth() {
        let mut history = History::default();
        history.remember("Move robin", 1);
        history.remember("Louder", 2);
        assert_eq!(history.undoable(), ["Move robin", "Louder"]);
        assert_eq!(history.undo(3), Some(2));
        assert_eq!(history.redoable(), ["Louder"]);
        assert_eq!(history.undo(2), Some(1));
        assert_eq!(history.undo(1), None);
        assert_eq!(history.redoable(), ["Move robin", "Louder"]);
        assert_eq!(history.redo(1), Some(2));
        assert_eq!(history.redo(2), Some(3));
        assert_eq!(history.redo(3), None);
        assert_eq!(history.undoable(), ["Move robin", "Louder"]);
    }

    /// Undoing what changed nothing would look like a broken undo.
    #[test]
    fn steps_that_changed_nothing_are_passed_over() {
        let mut history = History::default();
        history.remember("Move robin", 1);
        history.remember("Drag let go where it began", 2);
        assert_eq!(history.undo(2), Some(1));
        assert_eq!(history.redoable(), ["Move robin"]);
    }

    #[test]
    fn a_new_step_forgets_what_could_be_redone() {
        let mut history = History::default();
        history.remember("Move robin", 1);
        assert_eq!(history.undo(2), Some(1));
        history.remember("Louder", 1);
        assert!(history.redoable().is_empty());
    }

    #[test]
    fn only_the_newest_steps_are_kept() {
        let mut history = History::default();
        for version in 0..MOST_STEPS + 5 {
            history.remember("Step", version);
        }
        assert_eq!(history.undoable().len(), MOST_STEPS);
        assert_eq!(history.undo(usize::MAX), Some(MOST_STEPS + 4));
    }
}
