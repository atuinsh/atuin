//! The calls that the backend makes on the accessibility bus, and the names that it calls by.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future;
use zbus::message::Body;
use zbus::zvariant::{DynamicDeserialize, DynamicType};
use zbus::{Connection, ObjectPath, OwnedObjectPath, OwnedVariant, Variant};

use super::roles::AtspiRole;
use super::unlisted::Unlisted;
use crate::backend::Error;
use crate::layout::Rect;

pub const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
pub const APPLICATION: &str = "org.a11y.atspi.Application";
const CACHE: &str = "org.a11y.atspi.Cache";
const COLLECTION: &str = "org.a11y.atspi.Collection";
const COMPONENT: &str = "org.a11y.atspi.Component";
const DOCUMENT: &str = "org.a11y.atspi.Document";
const TEXT: &str = "org.a11y.atspi.Text";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// The bus daemon, which knows the process behind every connection.
pub const DBUS: &str = "org.freedesktop.DBus";
pub const DBUS_PATH: &str = "/org/freedesktop/DBus";

pub const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
/// The path that AT-SPI uses for a missing object. A top-level object may report this path as
/// its parent.
const NULL_PATH: &str = "/org/a11y/atspi/null";
pub const REGISTRY: &str = "org.a11y.atspi.Registry";
const CACHE_PATH: &str = "/org/a11y/atspi/cache";

/// The longest that one call may take. A hung application doesn't answer any calls.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(1);
/// How many elements are queried at once. More than this gains little speed.
pub const IN_FLIGHT: usize = 32;

/// The relation from an element to the element that labels it.
const RELATION_LABELLED_BY: u32 = 2;
/// The relation from a window to the document that the window shows.
const RELATION_EMBEDS: u32 = 13;
const MATCH_ALL: i32 = 1;
/// The order in which a Collection query walks: depth first, children in their own order.
const SORT_CANONICAL: u32 = 1;
/// The coordinates of the whole screen, as opposed to those of the window.
const SCREEN: u32 = 0;

pub const STATE_ACTIVE: u32 = 1;
pub const STATE_BUSY: u32 = 3;
pub const STATE_CHECKED: u32 = 4;
pub const STATE_ICONIFIED: u32 = 15;
pub const STATE_SHOWING: u32 = 25;
pub const STATE_VISIBLE: u32 = 30;

impl From<zbus::Error> for Error {
    fn from(error: zbus::Error) -> Self {
        Self::Platform(error.into())
    }
}

/// Whether `error` is an application's answer that it lacks the object, interface, property,
/// or method that a call asked for. Such an answer stays the same when the call is made again.
pub fn is_missing(error: &Error) -> bool {
    let Error::Platform(error) = error else {
        return false;
    };
    let Some(zbus::Error::MethodError(name, _, _)) = error.downcast_ref::<zbus::Error>() else {
        return false;
    };
    matches!(
        name.as_str(),
        "org.freedesktop.DBus.Error.UnknownObject"
            | "org.freedesktop.DBus.Error.UnknownInterface"
            | "org.freedesktop.DBus.Error.UnknownProperty"
            | "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.InvalidArgs"
            | "org.freedesktop.DBus.Error.NotSupported"
    )
}

/// The unique bus name of an application.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct App(Arc<str>);

impl App {
    pub(super) fn new(name: &str) -> Self {
        Self(Arc::from(name))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for App {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// An accessible object inside an application, such as a window.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Object {
    app: App,
    path: OwnedObjectPath,
}

impl Object {
    pub(super) fn new(app: App, path: ObjectPath<'_>) -> Self {
        Self {
            app,
            path: path.into(),
        }
    }

    pub(super) fn root(app: App) -> Self {
        Self::at_static_path(app, ROOT_PATH)
    }

    /// The bus daemon, which knows the process behind every connection.
    fn bus_daemon() -> Self {
        Self::at_static_path(App::new(DBUS), DBUS_PATH)
    }

    fn at_static_path(app: App, path: &'static str) -> Self {
        Self {
            app,
            path: OwnedObjectPath::from(ObjectPath::from_static_str_unchecked(path)),
        }
    }

    /// The object that a reference names, which `owner` sent as a bus name and a path.
    ///
    /// An empty bus name stands for `owner`.
    pub(super) fn reference(bus: &str, path: ObjectPath<'_>, owner: &App) -> Self {
        let app = if bus.is_empty() || bus == owner.as_str() {
            owner.clone()
        } else {
            App::new(bus)
        };
        Self {
            app,
            path: path.into(),
        }
    }

    /// Reads a reference that `owner` sent as a structure of a bus name and a path.
    pub(super) fn from_variant(value: &Variant<'_>, owner: &App) -> Option<Self> {
        let Variant::Structure(reference) = value else {
            return None;
        };
        let [Variant::Str(bus), Variant::ObjectPath(path)] = reference.fields() else {
            return None;
        };
        Some(Self::reference(bus.as_str(), path.as_ref(), owner))
    }

    #[must_use]
    pub const fn app(&self) -> &App {
        &self.app
    }

    #[must_use]
    pub fn into_app(self) -> App {
        self.app
    }

    #[must_use]
    pub fn path(&self) -> &str {
        self.path.as_str()
    }

    /// Whether the object is the root of an application or of the registry.
    pub(super) fn is_root(&self) -> bool {
        self.path() == ROOT_PATH
    }

    /// Whether the object stands above every window, as the parent of a window.
    ///
    /// That is the application's root, or the null object, which some applications name as the
    /// parent of a window instead.
    pub(super) fn is_above_windows(&self) -> bool {
        matches!(self.path(), ROOT_PATH | NULL_PATH)
    }
}

/// The UI toolkit of an application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolkit {
    /// The toolkit's name as the application gives it, such as `GTK`, `Qt`, `Gecko`, or
    /// `Chromium`.
    pub name: String,
    pub family: ToolkitFamily,
}

/// The toolkits that the backend tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolkitFamily {
    Gtk {
        major: u32,
    },
    Qt,
    Gecko,
    Chromium,
    Other,
}

impl ToolkitFamily {
    /// The family of the toolkit that an application names `name`, at `version`.
    pub fn of(name: &str, version: &str) -> Self {
        match name {
            "Gecko" => Self::Gecko,
            "Chromium" => Self::Chromium,
            "Qt" => Self::Qt,
            _ if name.eq_ignore_ascii_case("gtk") => {
                match version.split('.').next().and_then(|first| first.parse().ok()) {
                    Some(major) => Self::Gtk { major },
                    None => Self::Other,
                }
            }
            _ => Self::Other,
        }
    }
}

/// The accessibility bus, through one connection. Clones share the connection.
#[derive(Clone)]
pub struct Bus {
    conn: Connection,
}

impl Bus {
    pub const fn new(conn: Connection) -> Self {
        Self { conn }
    }

    /// Calls `method` on `object`, and hands the body of the reply to `read`, which can
    /// deserialize values that borrow from it.
    async fn call_with<B, T>(
        &self,
        object: &Object,
        interface: &str,
        method: &str,
        body: &B,
        read: impl FnOnce(&Body) -> zbus::Result<T>,
    ) -> Result<T, Error>
    where
        B: serde::Serialize + DynamicType + Sync,
    {
        let call = self.conn.call_method(
            Some(object.app.as_str()),
            object.path(),
            Some(interface),
            method,
            body,
        );
        let reply =
            tokio::time::timeout(CALL_TIMEOUT, call).await.map_err(|_| Error::NoAnswer)??;
        Ok(read(&reply.body())?)
    }

    pub async fn call<B, R>(
        &self,
        object: &Object,
        interface: &str,
        method: &str,
        body: &B,
    ) -> Result<R, Error>
    where
        B: serde::Serialize + DynamicType + Sync,
        R: for<'d> DynamicDeserialize<'d>,
    {
        self.call_with(object, interface, method, body, |body| body.deserialize()).await
    }

    /// Reads the property `name` of `interface` on `object`.
    async fn get<T>(&self, object: &Object, interface: &str, name: &str) -> Result<T, Error>
    where
        T: TryFrom<OwnedVariant, Error = zbus::Error>,
    {
        let value: OwnedVariant = self.call(object, PROPERTIES, "Get", &(interface, name)).await?;
        Ok(T::try_from(value)?)
    }

    pub async fn name(&self, object: &Object) -> Result<String, Error> {
        self.get(object, ACCESSIBLE, "Name").await
    }

    /// The toolkit of `app`. An application that doesn't tell the version of its toolkit counts
    /// as one with an unknown version.
    pub async fn toolkit(&self, app: &App) -> Result<Toolkit, Error> {
        let root = Object::root(app.clone());
        let (name, version) = future::join(
            self.get(&root, APPLICATION, "ToolkitName"),
            self.get(&root, APPLICATION, "Version"),
        )
        .await;
        let name: String = name?;
        let version = match version {
            Ok(version) => version,
            Err(error) if is_missing(&error) => String::new(),
            Err(error) => return Err(error),
        };
        Ok(Toolkit {
            family: ToolkitFamily::of(&name, &version),
            name,
        })
    }

    /// The id of the process that owns the connection of `app`.
    pub async fn connection_pid(&self, app: &App) -> Option<u32> {
        connection_pid(&self.conn, app.as_str(), CALL_TIMEOUT).await
    }

    /// The unique bus name that owns the well-known name `name`.
    pub async fn owner_of(&self, name: &str) -> Option<App> {
        self.call_with(&Object::bus_daemon(), DBUS, "GetNameOwner", &name, |body| {
            body.deserialize::<&str>().map(App::new)
        })
        .await
        .ok()
    }

    /// Finds the elements of `app` that GTK 4 leaves out of the tree. See
    /// [`quirks::leaves_roleless_elements_out`](super::quirks::leaves_roleless_elements_out).
    pub async fn unlisted(&self, app: &App) -> Unlisted {
        let cache = Object::at_static_path(app.clone(), CACHE_PATH);
        self.call(&cache, CACHE, "GetItems", &()).await.unwrap_or_default()
    }

    pub async fn children(&self, object: &Object) -> Result<Vec<Object>, Error> {
        self.call_with(object, ACCESSIBLE, "GetChildren", &(), |body| {
            let children: Vec<(&str, ObjectPath<'_>)> = body.deserialize()?;
            Ok(children
                .into_iter()
                .map(|(bus, path)| Object::reference(bus, path, &object.app))
                .collect())
        })
        .await
    }

    pub async fn state(&self, object: &Object) -> Result<Vec<u32>, Error> {
        self.call(object, ACCESSIBLE, "GetState", &()).await
    }

    pub async fn parent(&self, object: &Object) -> Result<Object, Error> {
        let parent = self
            .call_with(object, PROPERTIES, "Get", &(ACCESSIBLE, "Parent"), |body| {
                Ok(Object::from_variant(&body.deserialize::<Variant<'_>>()?, &object.app))
            })
            .await?;
        parent.ok_or_else(|| Error::Platform("the parent isn't a reference to an object".into()))
    }

    pub async fn role(&self, object: &Object) -> Result<AtspiRole, Error> {
        self.call(object, ACCESSIBLE, "GetRole", &()).await.map(AtspiRole)
    }

    /// The role's name in the toolkit's own words, such as `menu item radio`.
    pub async fn role_name(&self, object: &Object) -> Result<String, Error> {
        self.call(object, ACCESSIBLE, "GetRoleName", &()).await
    }

    /// The first target of the relation `kind` from `object`.
    async fn relation_target(&self, object: &Object, kind: u32) -> Option<Object> {
        self.call_with(object, ACCESSIBLE, "GetRelationSet", &(), |body| {
            let relations: Vec<(u32, Vec<(&str, ObjectPath<'_>)>)> = body.deserialize()?;
            Ok(relations
                .into_iter()
                .find(|(relation, _)| *relation == kind)
                .and_then(|(_, targets)| targets.into_iter().next())
                .map(|(bus, path)| Object::reference(bus, path, &object.app)))
        })
        .await
        .ok()
        .flatten()
    }

    /// The text of the element that labels `object`, for an element whose own name is empty.
    pub async fn label(&self, object: &Object) -> Option<String> {
        let label = self.relation_target(object, RELATION_LABELLED_BY).await?;
        match self.text(&label).await {
            Ok(text) if !text.trim().is_empty() => Some(text),
            _ => self.name(&label).await.ok(),
        }
    }

    /// Fetches the whole text of `object`.
    ///
    /// Firefox answers an end offset of -1 with an empty string, so the range is explicit.
    pub async fn text(&self, object: &Object) -> Result<String, Error> {
        let count = self.character_count(object).await?;
        if count == 0 {
            return Ok(String::new());
        }
        self.call(object, TEXT, "GetText", &(0i32, count)).await
    }

    pub async fn character_count(&self, object: &Object) -> Result<i32, Error> {
        self.get(object, TEXT, "CharacterCount").await
    }

    /// Finds the document that `window` shows. A browser points at the document of its active
    /// tab this way.
    ///
    /// The call is also what makes an application that builds its pages on request start
    /// doing so. See [`quirks::PAGE_BUILD_TIME`](super::quirks::PAGE_BUILD_TIME).
    pub async fn embedded_document(&self, window: &Object) -> Option<Object> {
        self.relation_target(window, RELATION_EMBEDS).await
    }

    /// Finds up to `count` elements below `ancestor` with every one of `attributes`, in depth-first
    /// order, and hands the references to them to `read`.
    ///
    /// A count of 0 asks for every element.
    ///
    /// The application searches its own tree, so the cost is one call however many elements
    /// there are.
    async fn get_matches<T>(
        &self,
        ancestor: &Object,
        attributes: HashMap<&str, &str>,
        count: i32,
        read: impl FnOnce(Vec<(&str, ObjectPath<'_>)>) -> T,
    ) -> Result<T, Error> {
        // The fields are states, attributes, roles, and interfaces, each with the way to match
        // them, and whether to invert the result. An empty set matches every element. Slices
        // serialize as arrays, and fixed-size arrays would serialize as structures.
        let no_states: &[i32] = &[0; 2];
        let no_roles: &[i32] = &[0; 4];
        let no_interfaces: &[&str] = &[];
        let rule = (
            no_states,
            MATCH_ALL,
            attributes,
            MATCH_ALL,
            no_roles,
            MATCH_ALL,
            no_interfaces,
            MATCH_ALL,
            false,
        );
        let traverse = true;
        self.call_with(
            ancestor,
            COLLECTION,
            "GetMatches",
            &(rule, SORT_CANONICAL, count, traverse),
            |body| Ok(read(body.deserialize()?)),
        )
        .await
    }

    /// Whether an element below `ancestor` has the attribute `key` with the value `value`.
    pub async fn has_attribute_below(
        &self,
        ancestor: &Object,
        key: &str,
        value: &str,
    ) -> Result<bool, Error> {
        self.get_matches(ancestor, HashMap::from([(key, value)]), 1, |found| !found.is_empty())
            .await
    }

    /// Lists `ancestor` and everything below it in depth-first order, with up to `limit`
    /// elements below it.
    pub async fn subtree(&self, ancestor: &Object, limit: usize) -> Result<Vec<Object>, Error> {
        let count = i32::try_from(limit.max(1)).unwrap_or(i32::MAX);
        self.get_matches(ancestor, HashMap::new(), count, |found| {
            let mut subtree = Vec::with_capacity(found.len() + 1);
            subtree.push(ancestor.clone());
            subtree.extend(
                found.into_iter().map(|(bus, path)| Object::reference(bus, path, &ancestor.app)),
            );
            subtree
        })
        .await
    }

    pub async fn child_count(&self, object: &Object) -> Result<usize, Error> {
        let count: i32 = self.get(object, ACCESSIBLE, "ChildCount").await?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Fetches the position of `object` on the screen, when it has one.
    pub async fn extents(&self, object: &Object) -> Option<Rect> {
        let (x, y, width, height): (i32, i32, i32, i32) =
            self.call(object, COMPONENT, "GetExtents", &SCREEN).await.ok()?;
        // GTK answers with the smallest integer for an element that doesn't have a position.
        if x == i32::MIN || y == i32::MIN {
            return None;
        }
        Some(Rect {
            x,
            y,
            width,
            height,
        })
    }

    /// The box of the character at `offset` in the text of `object`, in screen coordinates.
    pub async fn character_extents(&self, object: &Object, offset: usize) -> Option<Rect> {
        let offset = i32::try_from(offset).ok()?;
        let (x, y, width, height): (i32, i32, i32, i32) =
            self.call(object, TEXT, "GetCharacterExtents", &(offset, SCREEN)).await.ok()?;
        (width > 0 && height > 0).then_some(Rect {
            x,
            y,
            width,
            height,
        })
    }

    /// Whether `matches` accepts the value of the attribute `key` of `object`. `matches` gets
    /// `None` when `object` doesn't have the attribute.
    pub async fn attribute_matches(
        &self,
        object: &Object,
        key: &str,
        matches: impl FnOnce(Option<&str>) -> bool,
    ) -> Result<bool, Error> {
        self.call_with(object, ACCESSIBLE, "GetAttributes", &(), |body| {
            let attributes: HashMap<&str, &str> = body.deserialize()?;
            Ok(matches(attributes.get(key).copied()))
        })
        .await
    }

    pub async fn heading_level(&self, object: &Object) -> Option<u8> {
        self.call_with(object, ACCESSIBLE, "GetAttributes", &(), |body| {
            let attributes: HashMap<&str, &str> = body.deserialize()?;
            Ok(attributes.get("level").and_then(|level| level.parse().ok()))
        })
        .await
        .ok()
        .flatten()
    }

    pub async fn document_url(&self, object: &Object) -> Option<String> {
        self.call_with(object, DOCUMENT, "GetAttributes", &(), |body| {
            let attributes: HashMap<&str, &str> = body.deserialize()?;
            // Firefox names the attribute `DocURL`, and Chromium names it `URI`.
            Ok(["DocURL", "URI"]
                .iter()
                .find_map(|key| attributes.get(key))
                .filter(|url| !url.is_empty())
                .map(|url| (*url).to_owned()))
        })
        .await
        .ok()
        .flatten()
    }
}

/// The id of the process that owns the connection with the unique name `name`, on the bus of
/// `conn`.
///
/// Returns `None` when the bus doesn't answer within `timeout`, or when the connection already left
/// the bus.
pub async fn connection_pid(conn: &Connection, name: &str, timeout: Duration) -> Option<u32> {
    let call =
        conn.call_method(Some(DBUS), DBUS_PATH, Some(DBUS), "GetConnectionUnixProcessID", &name);
    let reply = tokio::time::timeout(timeout, call).await.ok()?.ok()?;
    reply.body().deserialize().ok()
}

pub fn is_active_window(state: &[u32]) -> bool {
    has_state(state, STATE_ACTIVE)
        && has_state(state, STATE_SHOWING)
        && !has_state(state, STATE_ICONIFIED)
}

pub fn has_state(state: &[u32], bit: u32) -> bool {
    state.get((bit / 32) as usize).is_some_and(|word| word & (1 << (bit % 32)) != 0)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::showing(&[1 << 25, 0], STATE_SHOWING, true)]
    #[case::not_showing(&[1 << 24, 0], STATE_SHOWING, false)]
    #[case::second_word(&[0, 1 << 3], 35, true)]
    #[case::short_state(&[], STATE_ACTIVE, false)]
    fn state_bits_are_read_across_words(
        #[case] state: &[u32],
        #[case] bit: u32,
        #[case] set: bool,
    ) {
        assert_eq!(has_state(state, bit), set);
    }

    #[rstest]
    #[case::active(&[STATE_ACTIVE, STATE_SHOWING], true)]
    #[case::inactive(&[STATE_SHOWING], false)]
    #[case::hidden(&[STATE_ACTIVE], false)]
    #[case::minimized(&[STATE_ACTIVE, STATE_SHOWING, STATE_ICONIFIED], false)]
    fn only_a_showing_window_is_active(#[case] bits: &[u32], #[case] active: bool) {
        let state = [bits.iter().fold(0, |word, bit| word | (1 << bit)), 0];
        assert_eq!(is_active_window(&state), active);
    }

    #[rstest]
    #[case::empty_name("", ":1.7")]
    #[case::same_name(":1.7", ":1.7")]
    #[case::other_name(":1.9", ":1.9")]
    fn a_reference_names_its_application(#[case] bus: &str, #[case] expected: &str) {
        let owner = App::new(":1.7");
        let path = ObjectPath::from_static_str_unchecked("/a11y/1");
        let object = Object::reference(bus, path, &owner);
        assert_eq!(object.app().as_str(), expected);
        assert_eq!(object.path(), "/a11y/1");
    }

    #[rstest]
    fn a_reference_shares_the_name_of_its_owner() {
        let owner = App::new(":1.7");
        let object =
            Object::reference(":1.7", ObjectPath::from_static_str_unchecked("/a11y/1"), &owner);
        assert!(Arc::ptr_eq(&object.app().0, &owner.0));
    }

    #[rstest]
    #[case::gtk3("GTK", "3.24.49", ToolkitFamily::Gtk { major: 3 })]
    #[case::gtk4("gtk", "4.18.6", ToolkitFamily::Gtk { major: 4 })]
    #[case::gtk40("GTK", "40.1", ToolkitFamily::Gtk { major: 40 })]
    #[case::gtk_without_a_version("GTK", "", ToolkitFamily::Other)]
    #[case::qt("Qt", "4.8", ToolkitFamily::Qt)]
    #[case::gecko("Gecko", "153.0", ToolkitFamily::Gecko)]
    #[case::chromium("Chromium", "156.0", ToolkitFamily::Chromium)]
    #[case::other("Elementary", "1.0", ToolkitFamily::Other)]
    fn toolkits_belong_to_their_family(
        #[case] name: &str,
        #[case] version: &str,
        #[case] expected: ToolkitFamily,
    ) {
        assert_eq!(ToolkitFamily::of(name, version), expected);
    }
}
