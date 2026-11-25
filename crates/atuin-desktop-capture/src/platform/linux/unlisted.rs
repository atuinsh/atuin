//! The elements that GTK 4 leaves out of the tree. See
//! [`quirks::leaves_roleless_elements_out`](super::quirks::leaves_roleless_elements_out).

use std::collections::HashMap;
use std::fmt;

use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, SeqAccess, Visitor};
use zbus::{ObjectPath, Signature, Type};

use super::bus::{App, Object};
use super::roles::AtspiRole;

/// The elements that GTK 4 leaves out of the children of their parents, by parent.
///
/// It deserializes from the reply to `GetItems` of the cache interface of an application, which
/// lists every element of the application with its role and its parent.
#[derive(Default)]
pub struct Unlisted(HashMap<Object, Vec<UnlistedChild>>);

struct UnlistedChild {
    /// How many listed children come before this one.
    index: usize,
    object: Object,
}

impl Unlisted {
    /// Puts the elements that belong under `parent` into `children`, the listed children of
    /// `parent`.
    pub fn insert_into(&self, parent: &Object, children: &mut Vec<Object>) {
        let Some(unlisted) = self.0.get(parent) else {
            return;
        };
        // The elements are sorted by index, so each insertion shifts the ones after it.
        for (inserted, child) in unlisted.iter().enumerate() {
            let at = (child.index + inserted).min(children.len());
            children.insert(at, child.object.clone());
        }
    }
}

impl Type for Unlisted {
    const SIGNATURE: &'static Signature = <Vec<CacheItem<'static>> as Type>::SIGNATURE;
}

impl<'de> Deserialize<'de> for Unlisted {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_seq(CacheItems)
    }
}

/// Keeps the elements without a role from a listing of every element.
struct CacheItems;

impl<'de> Visitor<'de> for CacheItems {
    type Value = Unlisted;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the elements in the cache of an application")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Unlisted, A::Error> {
        let mut by_parent = HashMap::<Object, Vec<UnlistedChild>>::new();
        while let Some(item) = items.next_element::<CacheItem<'de>>()? {
            if AtspiRole(item.role) != AtspiRole::INVALID {
                continue;
            }
            let Ok(index) = usize::try_from(item.index) else {
                continue;
            };
            let object = Object::new(App::new(item.object.bus), item.object.path);
            let parent = Object::reference(item.parent.bus, item.parent.path, object.app());
            by_parent.entry(parent).or_default().push(UnlistedChild { index, object });
        }
        for children in by_parent.values_mut() {
            children.sort_by_key(|child| child.index);
        }
        Ok(Unlisted(by_parent))
    }
}

/// One element in the reply to `GetItems`, with only the fields that the reader uses.
#[derive(Deserialize, Type)]
#[zvariant(signature = "((so)(so)(so)iiassusau)")]
struct CacheItem<'a> {
    #[serde(borrow)]
    object: Reference<'a>,
    _application: IgnoredAny,
    #[serde(borrow)]
    parent: Reference<'a>,
    index: i32,
    _child_count: IgnoredAny,
    _interfaces: IgnoredAny,
    _name: IgnoredAny,
    role: u32,
    _description: IgnoredAny,
    _states: IgnoredAny,
}

#[derive(Deserialize)]
struct Reference<'a> {
    bus: &'a str,
    #[serde(borrow)]
    path: ObjectPath<'a>,
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use zbus::wire::serialized::Context;
    use zbus::wire::{LE, to_bytes};

    use super::*;

    const APP: &str = ":1.7";

    /// The fields of one entry of `GetItems`, in the order of the cache interface.
    type Item = (
        (&'static str, ObjectPath<'static>),
        (&'static str, ObjectPath<'static>),
        (&'static str, ObjectPath<'static>),
        i32,
        i32,
        Vec<&'static str>,
        &'static str,
        u32,
        &'static str,
        Vec<u32>,
    );

    fn path(name: &str) -> ObjectPath<'static> {
        ObjectPath::try_from(format!("/a11y/{name}")).expect("the path is valid")
    }

    fn object(name: &str) -> Object {
        Object::new(App::new(APP), path(name))
    }

    fn item(name: &str, parent_bus: &'static str, parent: &str, index: i32, role: u32) -> Item {
        (
            (APP, path(name)),
            (APP, ObjectPath::from_static_str_unchecked("/org/a11y/atspi/accessible/root")),
            (parent_bus, path(parent)),
            index,
            1,
            vec!["org.a11y.atspi.Accessible"],
            "a name",
            role,
            "",
            vec![0, 0],
        )
    }

    /// Serializes `items` as an application sends them, and reads them back.
    fn unlisted(items: &[Item]) -> Unlisted {
        let context = Context::new(LE, 0);
        let data = to_bytes(context, items).expect("the items serialize");
        let (unlisted, _) = data.deserialize::<Unlisted>().expect("the items deserialize");
        unlisted
    }

    fn children_of(unlisted: &Unlisted, parent: &str, listed: &[&str]) -> Vec<Object> {
        let mut children = listed.iter().map(|name| object(name)).collect();
        unlisted.insert_into(&object(parent), &mut children);
        children
    }

    #[rstest]
    #[case::first(&[("row", 0)], &["a", "b"], &["row", "a", "b"])]
    #[case::middle(&[("row", 1)], &["a", "b"], &["a", "row", "b"])]
    #[case::last(&[("row", 2)], &["a", "b"], &["a", "b", "row"])]
    #[case::past_the_end(&[("row", 9)], &["a"], &["a", "row"])]
    #[case::same_place(&[("y", 1), ("x", 1)], &["a", "b"], &["a", "y", "x", "b"])]
    #[case::two_places(&[("y", 2), ("x", 0)], &["a", "b"], &["x", "a", "b", "y"])]
    #[case::no_listed_children(&[("row", 0)], &[], &["row"])]
    fn roleless_elements_join_their_listed_siblings(
        #[case] roleless: &[(&str, i32)],
        #[case] listed: &[&str],
        #[case] expected: &[&str],
    ) {
        let items: Vec<Item> = roleless
            .iter()
            .map(|&(name, index)| item(name, APP, "list", index, AtspiRole::INVALID.0))
            .collect();
        let unlisted = unlisted(&items);
        let expected: Vec<Object> = expected.iter().map(|name| object(name)).collect();
        assert_eq!(children_of(&unlisted, "list", listed), expected);
    }

    #[rstest]
    fn elements_with_a_role_stay_where_they_are() {
        let unlisted = unlisted(&[
            item("label", APP, "list", 0, AtspiRole::LABEL.0),
            item("row", APP, "list", 1, AtspiRole::INVALID.0),
        ]);
        assert_eq!(children_of(&unlisted, "list", &["a"]), vec![object("a"), object("row")]);
    }

    #[rstest]
    fn an_empty_bus_name_stands_for_the_application() {
        let unlisted = unlisted(&[item("row", "", "list", 0, AtspiRole::INVALID.0)]);
        assert_eq!(children_of(&unlisted, "list", &[]), vec![object("row")]);
    }

    #[rstest]
    fn other_parents_gain_nothing() {
        let unlisted = unlisted(&[item("row", APP, "list", 0, AtspiRole::INVALID.0)]);
        assert_eq!(children_of(&unlisted, "other", &["a"]), vec![object("a")]);
    }
}
