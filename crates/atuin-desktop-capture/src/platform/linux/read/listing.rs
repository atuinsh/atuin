//! Reads a document from one listing of its elements.

use std::collections::HashMap;

use atuin_common::string::insert_before_chars;
use futures::future;
use futures::stream::{self, StreamExt as _};
use tokio::time::Instant;

use super::budget::ListingBudget;
use super::fetch::{Indexed, in_order, optional};
use super::{RoleFailure, UnknownRoles, answered_role, is_web_terminal, option_mark};
use crate::backend::{Mark, Node, Role};
use crate::content::{has_words, uses_name};
use crate::flat::{Flat, Subtrees, assemble, parents};
use crate::layout::Frame;
use crate::platform::linux::bus::{Bus, IN_FLIGHT, Object};
use crate::platform::linux::normalize::{EMBEDDED, Layout, name_labels_children};
use crate::platform::linux::quirks;
use crate::platform::linux::roles::{AtspiRole, Within};

/// The elements of a document listing, each at its index.
struct Elements<'a> {
    objects: &'a [Object],
    shapes: &'a [Shape],
    subtrees: &'a Subtrees,
}

/// Where two children of `parent` meet, at `seam.offset` in the parent's text.
struct Junction {
    parent: usize,
    before: usize,
    after: usize,
    seam: quirks::Seam,
}

/// Whether each element of a listing is readable: its own role is known and readable, and its
/// parent is readable.
fn readable_elements(shapes: &[Shape], parents: &[Option<usize>]) -> Vec<bool> {
    // A parent always comes before its children, so one pass settles every element.
    let mut readable = Vec::with_capacity(shapes.len());
    for (shape, parent) in shapes.iter().zip(parents) {
        let own = shape.answer.is_ok() && shape.role.is_readable();
        readable.push(own && parent.is_none_or(|parent| readable[parent]));
    }
    readable
}

/// How far below the first element of a listing its deepest element hangs.
fn depth(parents: &[Option<usize>]) -> usize {
    // A parent always comes before its children, so one pass settles every element.
    let mut depths: Vec<usize> = Vec::with_capacity(parents.len());
    for parent in parents {
        depths.push(parent.map_or(0, |parent| depths[parent] + 1));
    }
    depths.into_iter().max().unwrap_or(0)
}

/// The junctions between the children of `parents` where Chromium may have left out a blank.
fn junctions(
    elements: &Elements<'_>,
    parents: &[usize],
    lengths: &HashMap<usize, usize>,
    content: &[Content],
) -> Vec<Junction> {
    let mut junctions = Vec::new();
    let mut children = Vec::new();
    for &parent in parents {
        children.clear();
        children.extend(elements.subtrees.children(parent));
        let pieces: Option<Vec<quirks::Piece<'_>>> = children
            .iter()
            .map(|&child| {
                if elements.shapes[child].is_run() {
                    let characters = *lengths.get(&child)?;
                    Some(quirks::Piece::Run { characters })
                } else {
                    let text = content[child].text.as_deref();
                    Some(quirks::Piece::Embedded { text })
                }
            })
            .collect();
        let text = content[parent].text.as_deref().unwrap_or_default();
        let Some(seams) = pieces.and_then(|pieces| quirks::seams(text, &pieces)) else {
            continue;
        };
        junctions.extend(seams.into_iter().map(|seam| Junction {
            parent,
            before: children[seam.before],
            after: children[seam.after],
            seam,
        }));
    }
    junctions
}

/// Builds the tree of a document from the shapes and the content of its listing.
fn assemble_listing(shapes: &[Shape], content: Vec<Content>) -> Option<Node> {
    let listing = shapes.iter().zip(content).map(|(shape, content)| Flat {
        node: Node {
            role: shape.role,
            native_role: shape.atspi().map(AtspiRole::native_name).unwrap_or_default(),
            url: content.url,
            name: content.name,
            text: content.text,
            heading_level: content.heading_level,
            mark: content.mark,
            ..Node::default()
        },
        children: shape.children,
    });
    assemble(listing).ok()
}

/// A document that was read from a listing of its elements.
pub struct Listed {
    pub document: Node,
    /// How many elements the document has, the document included.
    pub nodes: usize,
    /// The elements that did not tell their role, which the document leaves out.
    pub unknown_roles: UnknownRoles,
}

/// What places an element of a document listing in the tree.
struct Shape {
    /// The role of the element, or why the element did not tell its role.
    answer: Result<AtspiRole, RoleFailure>,
    role: Role,
    children: usize,
}

impl Shape {
    /// The role of the element, when the element answered for it.
    fn atspi(&self) -> Option<AtspiRole> {
        self.answer.ok()
    }

    /// Whether the element is a plain run of text. See
    /// [`normalize::is_run`](crate::platform::linux::normalize::is_run).
    fn is_run(&self) -> bool {
        self.atspi() == Some(AtspiRole::STATIC) && self.children == 0
    }

    /// Whether the element is a check box or a radio button.
    fn is_option(&self) -> bool {
        self.atspi().is_some_and(|atspi| atspi.is_option(Within::Document))
    }
}

/// The [`Layout`] of every element of a document listing that has text.
struct Layouts(Vec<Option<Layout>>);

impl Layouts {
    fn new(shapes: &[Shape], subtrees: &Subtrees, content: &[Content]) -> Self {
        let layouts = content.iter().zip(shapes).enumerate().map(|(index, (content, shape))| {
            let text = content.text.as_deref()?;
            let runs = subtrees.children(index).filter(|&child| shapes[child].is_run()).count();
            Some(Layout::of(text, shape.children, runs))
        });
        Self(layouts.collect())
    }

    fn of(&self, index: usize) -> Option<Layout> {
        self.0[index]
    }
}

/// What a readable element of a document listing holds.
#[derive(Default)]
struct Content {
    /// The URL, when the element is a document inside the listed one, such as a frame.
    url: Option<String>,
    name: String,
    text: Option<String>,
    heading_level: Option<u8>,
    mark: Mark,
}

/// Reads documents from listings of their elements, within the limits of one read.
pub struct Listing<'a> {
    bus: &'a Bus,
    deadline: Instant,
    /// The elements that the listing may add below the document.
    budget: &'a ListingBudget,
    /// How deep the tree of the document may nest, with the document at depth 0.
    max_depth: usize,
}

impl<'a> Listing<'a> {
    pub const fn new(
        bus: &'a Bus,
        deadline: Instant,
        budget: &'a ListingBudget,
        max_depth: usize,
    ) -> Self {
        Self {
            bus,
            deadline,
            budget,
            max_depth,
        }
    }
}

impl Listing<'_> {
    /// Reads the whole of `document`, whose role is `role`, from one listing of its
    /// elements.
    ///
    /// The listing makes every element known at once, so each round of calls goes out for
    /// all elements together, and only the elements that hold content are asked for text.
    /// Returns nothing when the document is too large or too deep for the limits, when the
    /// document changes during the read, or when the application cannot list its elements.
    /// The caller then walks the document instead.
    pub async fn read_document(&self, document: &Object, role: AtspiRole) -> Option<Listed> {
        let available = self.budget.available();
        // One element more than the budget tells that the document does not fit.
        let objects = self.bus.subtree(document, available + 1).await.ok()?;
        let reservation = self.budget.reserve(objects.len() - 1)?;
        let objects = &objects;
        // The listing starts with the document itself.
        let mut shapes = in_order(
            self.for_each(0..objects.len(), |index| {
                self.fetch_shape(&objects[index], (index == 0).then_some(role))
            })
            .await,
        );
        self.mark_web_terminals(objects, &mut shapes).await;
        let mut unknown_roles = UnknownRoles::default();
        for failure in shapes.iter().filter_map(|shape| shape.answer.err()) {
            unknown_roles.add(failure);
        }
        let parents = parents(shapes.iter().map(|shape| shape.children)).ok()?;
        if depth(&parents) > self.max_depth {
            return None;
        }
        let readable = readable_elements(&shapes, &parents);
        if Instant::now() >= self.deadline {
            return None;
        }

        let subtrees = Subtrees::new(&parents);
        let elements = Elements {
            objects,
            shapes: &shapes,
            subtrees: &subtrees,
        };
        // Chromium repeats a plain run of text inside the text of its parent. The text of the
        // parent tells whether a run needs a call of its own, so the runs come second.
        let (runs, others): (Vec<usize>, Vec<usize>) = (0..objects.len())
            .filter(|&index| readable[index])
            .partition(|&index| shapes[index].is_run());
        let mut content: Vec<Content> = Vec::new();
        content.resize_with(objects.len(), Content::default);
        self.fetch_contents_into(&elements, &others, &mut content).await;
        let layouts = Layouts::new(&shapes, &subtrees, &content);
        let runs: Vec<usize> = runs
            .into_iter()
            .filter(|&index| {
                parents[index].is_none_or(|parent| layouts.of(parent) != Some(Layout::RunsInText))
            })
            .collect();
        self.fetch_contents_into(&elements, &runs, &mut content).await;
        if Instant::now() >= self.deadline {
            return None;
        }
        self.restore_blanks(&elements, &layouts, &mut content).await;
        if Instant::now() >= self.deadline {
            return None;
        }
        let document = assemble_listing(&shapes, content)?;
        reservation.keep();
        Some(Listed {
            document,
            nodes: objects.len(),
            unknown_roles,
        })
    }

    /// Gives the terminals of web pages among the elements of a listing the role of a
    /// terminal. See [`is_web_terminal`].
    async fn mark_web_terminals(&self, objects: &[Object], shapes: &mut [Shape]) {
        let candidates = (0..shapes.len())
            .filter(|&index| shapes[index].atspi().is_some_and(quirks::may_be_web_terminal));
        let found = self.for_each(candidates, |index| is_web_terminal(self.bus, &objects[index]));
        for Indexed { index, value } in found.await {
            let shape = &mut shapes[index];
            match value {
                Ok(true) => shape.role = Role::Terminal,
                Ok(false) => {}
                Err(failure) => {
                    shape.answer = Err(failure);
                    shape.role = Role::Container;
                }
            }
        }
    }

    /// Fetches the content of the elements at `indices` into `content`.
    async fn fetch_contents_into(
        &self,
        elements: &Elements<'_>,
        indices: &[usize],
        content: &mut [Content],
    ) {
        let fetched = self
            .for_each(indices.iter().copied(), |index| {
                self.fetch_content(&elements.objects[index], &elements.shapes[index])
            })
            .await;
        for Indexed { index, value } in fetched {
            content[index] = value;
        }
    }

    /// Puts back the blanks that Chromium leaves out of the text of an element, where the
    /// page shows them between two of the element's children. See [`quirks::Piece`].
    async fn restore_blanks(
        &self,
        elements: &Elements<'_>,
        layouts: &Layouts,
        content: &mut [Content],
    ) {
        // Only an element that repeats its runs inside its own text can lose a blank there.
        let parents_with_runs: Vec<usize> = (0..elements.objects.len())
            .filter(|&index| {
                layouts.of(index) == Some(Layout::RunsInText)
                    && content[index].text.as_ref().is_some_and(|text| {
                        quirks::may_lack_blanks(text, elements.shapes[index].children)
                    })
            })
            .collect();
        if parents_with_runs.is_empty() {
            return;
        }
        let lengths = self.run_lengths(elements, &parents_with_runs).await;
        let junctions = junctions(elements, &parents_with_runs, &lengths, content);
        let apart = self.measure_gaps(elements, &junctions).await;
        let mut start = 0;
        for group in junctions.chunk_by(|a, b| a.parent == b.parent) {
            let gaps = &apart[start..start + group.len()];
            start += group.len();
            let offsets = group
                .iter()
                .zip(gaps)
                .filter(|(_, apart)| **apart)
                .map(|(junction, _)| junction.seam.offset);
            if let Some(text) = &mut content[group[0].parent].text {
                *text = insert_before_chars(text, ' ', offsets);
            }
        }
    }

    /// The number of characters of every run of text below `parents`, by index.
    async fn run_lengths(
        &self,
        elements: &Elements<'_>,
        parents: &[usize],
    ) -> HashMap<usize, usize> {
        let runs: Vec<usize> = parents
            .iter()
            .flat_map(|&parent| elements.subtrees.children(parent))
            .filter(|&index| elements.shapes[index].is_run())
            .collect();
        let counts =
            self.for_each(runs, |index| self.bus.character_count(&elements.objects[index])).await;
        counts
            .into_iter()
            .filter_map(|Indexed { index, value }| {
                Some((index, usize::try_from(value.ok()?).ok()?))
            })
            .collect()
    }

    /// Whether the page shows room for a blank at each junction.
    async fn measure_gaps(&self, elements: &Elements<'_>, junctions: &[Junction]) -> Vec<bool> {
        let gaps = self.for_each(0..junctions.len(), |index| {
            let junction = &junctions[index];
            async move {
                let (before, after) = future::join(
                    self.bus
                        .character_extents(&elements.objects[junction.before], junction.seam.last),
                    self.bus.character_extents(&elements.objects[junction.after], 0),
                )
                .await;
                before.zip(after).is_some_and(|(before, after)| {
                    Frame::from(before).leaves_room_for(after.into())
                })
            }
        });
        in_order(gaps.await)
    }

    /// Fetches what places an element in the tree. `known_role` is the role of the element
    /// when the caller already has it.
    async fn fetch_shape(&self, object: &Object, known_role: Option<AtspiRole>) -> Shape {
        let role = async {
            match known_role {
                Some(role) => Ok(role),
                None => answered_role(self.bus.role(object).await),
            }
        };
        let (answer, children) = future::join(role, self.bus.child_count(object)).await;
        Shape {
            answer,
            role: answer.map_or(Role::Container, AtspiRole::role),
            children: children.unwrap_or(0),
        }
    }

    /// Fetches what a readable element holds.
    async fn fetch_content(&self, object: &Object, shape: &Shape) -> Content {
        if shape.role == Role::Secret {
            return Content::default();
        }
        let (text, heading_level, state, url) = future::join4(
            self.bus.text(object),
            optional((shape.role == Role::Heading).then(|| self.bus.heading_level(object))),
            optional(shape.is_option().then_some(async { self.bus.state(object).await.ok() })),
            optional((shape.role == Role::Document).then(|| self.bus.document_url(object))),
        )
        .await;
        let mut text = text.ok();
        if shape.role == Role::ListItem
            && let Some(text) = &mut text
        {
            quirks::strip_list_bullet(text);
        }
        let needs_name = text.as_deref().is_none_or(|text| {
            !text.split(EMBEDDED).any(has_words) && !name_labels_children(shape.role, text)
        });
        let name = if needs_name && uses_name(shape.role) {
            self.bus.name(object).await.unwrap_or_default()
        } else {
            String::new()
        };
        Content {
            url,
            name,
            text,
            heading_level,
            mark: state.map_or(Mark::None, |state| option_mark(&state)),
        }
    }

    /// Runs `fetch` for every index, many at a time, and returns each result with its index,
    /// in the order in which the results arrived.
    ///
    /// The indices are passed by value, because a stream over borrowed items is not `Send`.
    async fn for_each<T, F>(
        &self,
        indices: impl IntoIterator<Item = usize>,
        fetch: impl Fn(usize) -> F,
    ) -> Vec<Indexed<T>>
    where
        F: Future<Output = T>,
    {
        stream::iter(indices)
            .map(|index| {
                let result = fetch(index);
                async move {
                    Indexed {
                        index,
                        value: result.await,
                    }
                }
            })
            .buffer_unordered(IN_FLIGHT)
            .collect()
            .await
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::one_element(&[None], 0)]
    #[case::flat(&[None, Some(0), Some(0)], 1)]
    #[case::nested(&[None, Some(0), Some(1), Some(2), Some(0)], 3)]
    fn the_depth_is_that_of_the_deepest_element(
        #[case] parents: &[Option<usize>],
        #[case] expected: usize,
    ) {
        assert_eq!(depth(parents), expected);
    }

    fn shape(atspi: Option<AtspiRole>, children: usize) -> Shape {
        Shape {
            answer: atspi.ok_or(RoleFailure::Failed),
            role: atspi.map_or(Role::Container, AtspiRole::role),
            children,
        }
    }

    #[rstest]
    fn an_element_without_a_role_is_read_as_nothing() {
        let shapes = [
            shape(Some(AtspiRole::DOCUMENT_WEB), 2),
            shape(None, 1),
            shape(Some(AtspiRole::PARAGRAPH), 0),
            shape(Some(AtspiRole::PARAGRAPH), 0),
        ];
        let parents = [None, Some(0), Some(1), Some(0)];
        assert_eq!(readable_elements(&shapes, &parents), [true, false, false, true]);
    }

    /// The shape of xterm.js in VS Code: the terminal element, its accessibility tree, and a row.
    #[rstest]
    fn a_terminal_is_read_as_nothing() {
        let mut terminal = shape(Some(AtspiRole::SECTION), 1);
        terminal.role = Role::Terminal;
        let shapes = [
            shape(Some(AtspiRole::DOCUMENT_WEB), 2),
            terminal,
            shape(Some(AtspiRole::SECTION), 1),
            shape(Some(AtspiRole::LIST_ITEM), 0),
            shape(Some(AtspiRole::PARAGRAPH), 0),
        ];
        let parents = [None, Some(0), Some(1), Some(2), Some(0)];
        assert_eq!(readable_elements(&shapes, &parents), [true, false, false, false, true]);
    }
}
