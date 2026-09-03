//! Tasks.
//!
//! A task is a block, not a separate kind of object (architecture §10.4). Its due
//! date, estimate and owner are ordinary inline fields on the same block, so they
//! index, query and write back through exactly the same machinery as every other
//! field. This file adds only what a checkbox has that a list item does not.

use serde::{Deserialize, Serialize};

use crate::span::ByteRange;

/// One checkbox item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// The block this task is.
    pub block_ord: u32,
    /// Whether the box is ticked. `ix_tasks_open` is partial on `done = 0`, which is
    /// the only side anyone queries (architecture §4.4).
    pub done: bool,
    /// The character inside the brackets: `' '`, `'x'`, or one of the alternative
    /// states Obsidian users write (`/`, `-`, `>`). Preserved because rewriting
    /// someone's `[/]` as `[ ]` would be a change they did not ask for.
    pub marker: char,
    /// Task text with the checkbox and list marker removed.
    pub text: String,
    /// The whole task line.
    pub range: ByteRange,
    /// Just the character between the brackets. Toggling a task replaces exactly one
    /// byte, which is the smallest possible instance of ADR-0008.
    pub marker_range: ByteRange,
}

impl Task {
    /// Whether a marker character counts as done.
    ///
    /// Only `x` and `X` are done. `/`, `-` and `>` are in-progress, cancelled and
    /// deferred by convention, and treating any of them as done would silently drop
    /// rows from every open-task query.
    #[must_use]
    pub const fn marker_is_done(marker: char) -> bool {
        matches!(marker, 'x' | 'X')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_x_is_done() {
        assert!(Task::marker_is_done('x'));
        assert!(Task::marker_is_done('X'));
        for open in [' ', '/', '-', '>', '?'] {
            assert!(!Task::marker_is_done(open), "{open:?} counted as done");
        }
    }
}
