//! Lists the raw element tree of a window for `doctor --outline`.

use std::collections::HashSet;

use futures::future;

use super::bus::{Bus, Object, STATE_SHOWING, has_state};
use super::roles::Within;
use crate::backend::{Completeness, Error, Role};
use crate::layout::Frame;
use crate::outline::{Outline, OutlineEntry, Skip};

/// Lists the elements of `window` in depth-first order, `max_entries` at most.
pub async fn outline(bus: &Bus, window: &Object, max_entries: usize) -> Result<Outline, Error> {
    let mut entries = Vec::new();
    let mut completeness = Completeness::Complete;
    let mut visited = HashSet::from([window.clone()]);
    let mut stack = vec![OutlineStep {
        depth: 0,
        object: window.clone(),
        within: Within::Window,
    }];
    while let Some(OutlineStep {
        depth,
        object,
        within,
    }) = stack.pop()
    {
        if entries.len() == max_entries {
            completeness = Completeness::Truncated;
            break;
        }
        let element = outline_element(bus, &object, within, depth).await;
        // A window that is gone fails here. Deeper elements may come and go.
        if entries.is_empty() && !element.answered {
            return Err(Error::NoAnswer);
        }
        entries.push(element.entry);
        let inner = element.inner;
        // An application can list an element below itself, and the outline would never
        // end.
        let unvisited =
            element.children.into_iter().rev().filter(|child| visited.insert(child.clone()));
        stack.extend(unvisited.map(|object| OutlineStep {
            depth: depth + 1,
            object,
            within: inner,
        }));
    }
    Ok(Outline {
        entries,
        completeness,
    })
}

/// An element that an outline waits to visit.
struct OutlineStep {
    depth: usize,
    object: Object,
    within: Within,
}

/// One element of an outline, with what the outline needs to go on below it.
struct OutlinedElement {
    entry: OutlineEntry,
    children: Vec<Object>,
    /// Where the children lie.
    inner: Within,
    /// Whether the element answered for its state.
    answered: bool,
}

/// Fetches one element for an outline, at `depth` below the window and `within` the elements
/// around it.
async fn outline_element(
    bus: &Bus,
    object: &Object,
    within: Within,
    depth: usize,
) -> OutlinedElement {
    let ((state, role, role_name, characters, children), extents) = future::join(
        future::join5(
            bus.state(object),
            bus.role(object),
            bus.role_name(object),
            bus.character_count(object),
            bus.children(object),
        ),
        bus.extents(object),
    )
    .await;
    let atspi = role.ok();
    let role = atspi.map_or(Role::Container, |role| within.role(role));
    let children = children.unwrap_or_default();
    let showing = state.as_ref().is_ok_and(|state| has_state(state, STATE_SHOWING));
    OutlinedElement {
        entry: OutlineEntry {
            depth,
            native_role: role_name.unwrap_or_else(|_| "?".to_owned()),
            role,
            characters: characters.ok().and_then(|count| usize::try_from(count).ok()),
            frame: extents.map(Frame::from),
            children: children.len(),
            skipped: if !showing {
                Some(Skip::Hidden)
            } else if atspi.is_none() {
                Some(Skip::UnknownRole)
            } else if role.is_readable() {
                None
            } else {
                Some(Skip::Unreadable)
            },
        },
        children,
        inner: atspi.map_or(within, |role| within.inner(role)),
        answered: state.is_ok(),
    }
}
