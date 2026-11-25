//! Rebuilds a tree from its elements in depth-first order.

use crate::backend::Node;

/// One element of a depth-first listing.
#[derive(Debug, Default)]
pub struct Flat {
    /// The element without its children.
    pub node: Node,
    /// How many of the elements that follow are children of this element.
    pub children: usize,
}

/// The listing does not describe one whole tree.
#[derive(Debug, PartialEq, Eq)]
pub struct Inconsistent;

struct Pending {
    node: Node,
    missing: usize,
}

/// Builds the tree that `listing` describes.
///
/// Fails when the child counts ask for more elements than the listing holds, or when elements
/// are left over after the tree is complete.
pub fn assemble(listing: impl IntoIterator<Item = Flat>) -> Result<Node, Inconsistent> {
    let mut open: Vec<Pending> = Vec::new();
    let mut root = None;
    for flat in listing {
        if root.is_some() {
            return Err(Inconsistent);
        }
        open.push(Pending {
            node: flat.node,
            missing: flat.children,
        });
        while let Some(done) = open.pop_if(|top| top.missing == 0) {
            match open.last_mut() {
                Some(parent) => {
                    parent.node.children.push(done.node);
                    parent.missing -= 1;
                }
                None => root = Some(done.node),
            }
        }
    }
    root.filter(|_| open.is_empty()).ok_or(Inconsistent)
}

/// Finds the parent of every element of a depth-first listing from the child counts.
///
/// The first element is the root and has no parent. Fails under the same conditions as
/// [`assemble`].
pub fn parents(
    child_counts: impl IntoIterator<Item = usize>,
) -> Result<Vec<Option<usize>>, Inconsistent> {
    struct Open {
        index: usize,
        missing: usize,
    }

    let mut parents = Vec::new();
    let mut open: Vec<Open> = Vec::new();
    for (index, missing) in child_counts.into_iter().enumerate() {
        match open.last_mut() {
            Some(parent) => {
                parents.push(Some(parent.index));
                parent.missing -= 1;
            }
            None if index == 0 => parents.push(None),
            None => return Err(Inconsistent),
        }
        open.push(Open { index, missing });
        while open.pop_if(|top| top.missing == 0).is_some() {}
    }
    if open.is_empty() && !parents.is_empty() {
        Ok(parents)
    } else {
        Err(Inconsistent)
    }
}

/// The ends of the subtrees of a depth-first listing, for walking the children of an element.
pub struct Subtrees {
    /// For each element, the index that follows the last element of its subtree.
    ends: Vec<usize>,
}

impl Subtrees {
    /// Finds the subtrees from the parents that [`parents`] found.
    pub fn new(parents: &[Option<usize>]) -> Self {
        let mut ends: Vec<usize> = (1..=parents.len()).collect();
        // A child comes after its parent, so a backward pass settles each child first.
        for (index, parent) in parents.iter().enumerate().rev() {
            if let Some(parent) = *parent {
                ends[parent] = ends[parent].max(ends[index]);
            }
        }
        Self { ends }
    }

    /// The indices of the children of the element at `index`, in order.
    pub fn children(&self, index: usize) -> impl Iterator<Item = usize> {
        let end = self.ends[index];
        std::iter::successors(Some(index + 1), |&child| self.ends.get(child).copied())
            .take_while(move |&child| child < end)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn flat(name: &str, children: usize) -> Flat {
        Flat {
            node: Node {
                name: name.to_owned(),
                ..Node::default()
            },
            children,
        }
    }

    /// Writes a tree as nested names, such as `a(b c(d))`.
    fn outline(node: &Node, out: &mut String) {
        out.push_str(&node.name);
        if !node.children.is_empty() {
            out.push('(');
            for (i, child) in node.children.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                outline(child, out);
            }
            out.push(')');
        }
    }

    #[rstest]
    #[case::leaf(&[("a", 0)], Some("a"))]
    #[case::nested(&[("a", 2), ("b", 0), ("c", 1), ("d", 0)], Some("a(b c(d))"))]
    #[case::deep_then_sibling(&[("a", 2), ("b", 1), ("c", 1), ("d", 0), ("e", 0)], Some("a(b(c(d)) e)"))]
    #[case::too_few_elements(&[("a", 2), ("b", 0)], None)]
    #[case::left_over_elements(&[("a", 1), ("b", 0), ("c", 0)], None)]
    #[case::empty(&[], None)]
    fn listings_become_trees(#[case] listing: &[(&str, usize)], #[case] expected: Option<&str>) {
        let tree = assemble(listing.iter().map(|&(name, children)| flat(name, children)));
        let written = tree.map(|root| {
            let mut out = String::new();
            outline(&root, &mut out);
            out
        });
        assert_eq!(written.as_deref().ok(), expected);
    }

    #[rstest]
    #[case::leaf(&[0], Some(&[None][..]))]
    #[case::nested(&[2, 0, 1, 0], Some(&[None, Some(0), Some(0), Some(2)][..]))]
    #[case::deep_then_sibling(&[2, 1, 1, 0, 0], Some(&[None, Some(0), Some(1), Some(2), Some(0)][..]))]
    #[case::too_few_elements(&[2, 0], None)]
    #[case::left_over_elements(&[1, 0, 0], None)]
    #[case::empty(&[], None)]
    fn child_counts_give_parents(
        #[case] child_counts: &[usize],
        #[case] expected: Option<&[Option<usize>]>,
    ) {
        let found = parents(child_counts.iter().copied());
        assert_eq!(found.as_deref().ok(), expected);
    }

    #[rstest]
    #[case::leaf(&[0], 0, &[])]
    #[case::root(&[2, 1, 1, 0, 0], 0, &[1, 4])]
    #[case::inner(&[3, 2, 0, 0, 0, 1, 0], 1, &[2, 3])]
    #[case::after_a_deep_sibling(&[3, 1, 1, 0, 0, 1, 0], 0, &[1, 4, 5])]
    fn subtrees_give_children(
        #[case] child_counts: &[usize],
        #[case] index: usize,
        #[case] expected: &[usize],
    ) {
        let parents = parents(child_counts.iter().copied()).expect("the listing is one tree");
        let children: Vec<usize> = Subtrees::new(&parents).children(index).collect();
        assert_eq!(children, expected);
    }
}
