//! Follows the notifications that applications send to the notification service of the session.
//!
//! The session bus carries every notification on its way to the service, regardless of what
//! draws the notification: some services are not on the accessibility bus, and others draw a
//! notification as a window that names only the service. A monitor connection sees the call
//! and its sender.

use std::fmt;
use std::marker::PhantomData;
use std::time::Duration;

use futures::future::{self, BoxFuture};
use futures::{FutureExt as _, StreamExt as _};
use parking_lot::Mutex;
use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
use zbus::names::UniqueName;
use zbus::zvariant::Type;
use zbus::{Connection, MessageStream, Variant, message};

use super::bus::{DBUS, DBUS_PATH, connection_pid};
use crate::backend::Notice;

/// The calls that post a notification: the freedesktop interface, and the interface of GLib's
/// `GNotification`, which GNOME Shell serves. GLib uses its own interface only when a service
/// serves it, so the two never carry the same notification.
const RULES: [&str; 2] = [
    "type='method_call',interface='org.freedesktop.Notifications',member='Notify'",
    "type='method_call',interface='org.gtk.Notifications',member='AddNotification'",
];

/// The well-known name of the notification service.
const SERVICE: &str = "org.freedesktop.Notifications";

/// How long the bus may take to answer a question about the sender of a notification.
const QUERY_TIMEOUT: Duration = Duration::from_millis(500);

/// A connection that receives a copy of every notification sent on the session bus.
pub struct Notices {
    messages: MessageStream,
    /// A connection that asks the bus about senders. A monitor connection can't make calls.
    queries: Connection,
    /// The notification whose sender is still to be looked up, with the lookup, which keeps
    /// its progress when [`next`](Self::next) is cancelled.
    ///
    /// The mutex makes the connection `Sync`. Only `&mut self` reaches the lookup, so the
    /// mutex is never locked.
    pending: Option<Mutex<BoxFuture<'static, Option<Notice>>>>,
}

impl Notices {
    pub async fn watch() -> Result<Self, zbus::Error> {
        let conn = Connection::session().await?;
        conn.call_method(
            Some(DBUS),
            DBUS_PATH,
            Some("org.freedesktop.DBus.Monitoring"),
            "BecomeMonitor",
            &(&RULES[..], 0_u32),
        )
        .await?;
        Ok(Self {
            messages: MessageStream::from(conn),
            queries: Connection::session().await?,
            pending: None,
        })
    }

    /// Waits for the next notification. Cancelling the wait doesn't lose any notification.
    pub async fn next(&mut self) -> Option<Notice> {
        loop {
            if let Some(pending) = &mut self.pending {
                let notice = pending.get_mut().await;
                self.pending = None;
                match notice {
                    Some(notice) => return Some(notice),
                    None => continue,
                }
            }
            let Ok(msg) = self.messages.next().await? else {
                continue;
            };
            let Some(notice) = parse(&msg) else {
                continue;
            };
            let header = msg.header();
            let Some(sender) = header.sender().map(UniqueName::to_owned) else {
                return Some(notice);
            };
            let route = Route {
                sender,
                destination: header.destination().map(|destination| destination.to_string()),
            };
            self.pending =
                Some(Mutex::new(from_sender(self.queries.clone(), route, notice).boxed()));
        }
    }
}

/// The connection that sent a notification, and the name that the notification went to.
struct Route {
    sender: UniqueName<'static>,
    destination: Option<String>,
}

/// Fills in the process of `notice`, which went along `route`. Returns `None` for the copy of
/// a notification that the notification service passes on.
async fn from_sender(queries: Connection, route: Route, mut notice: Notice) -> Option<Notice> {
    let sender = route.sender.as_str();
    let (owner, pid) =
        future::join(service_owner(&queries), connection_pid(&queries, sender, QUERY_TIMEOUT))
            .await;
    if is_passed_on(sender, route.destination.as_deref(), owner.as_deref()) {
        return None;
    }
    notice.pid = pid;
    Some(notice)
}

/// Whether a notification that `sender` sent to `destination` is the copy that the
/// notification service passes on, when the connection `owner` owns the service's name.
///
/// GNOME Shell serves the freedesktop interface in a process of its own,
/// `org.gnome.Shell.Notifications`, which passes each call on to the shell, so the bus carries
/// each notification twice. The copy comes from the connection that owns the service's name,
/// and goes to the shell. A notification that the service posts to itself goes to the
/// service's name or to the owner, and is kept. Plasma 6.6 doesn't put such a notification on
/// the bus at all, because Qt delivers a call to a name of the calling connection in the
/// process.
fn is_passed_on(sender: &str, destination: Option<&str>, owner: Option<&str>) -> bool {
    owner == Some(sender)
        && destination.is_some_and(|destination| destination != SERVICE && destination != sender)
}

/// The connection that owns the name of the notification service, when the bus tells in time.
async fn service_owner(queries: &Connection) -> Option<String> {
    let call = queries.call_method(Some(DBUS), DBUS_PATH, Some(DBUS), "GetNameOwner", &SERVICE);
    let reply = tokio::time::timeout(QUERY_TIMEOUT, call).await.ok()?.ok()?;
    reply.body().deserialize::<String>().ok()
}

/// The arguments of `org.freedesktop.Notifications.Notify`.
#[derive(Deserialize, Type)]
#[zvariant(signature = "susssasa{sv}i")]
struct Notify<'a> {
    app: &'a str,
    _replaces_id: IgnoredAny,
    _icon: IgnoredAny,
    title: &'a str,
    body: &'a str,
    _actions: IgnoredAny,
    hints: Picked<'a, DesktopEntry, 1>,
    _timeout: IgnoredAny,
}

/// The arguments of `org.gtk.Notifications.AddNotification`.
#[derive(Deserialize, Type)]
#[zvariant(signature = "ssa{sv}")]
struct AddNotification<'a> {
    app: &'a str,
    _id: IgnoredAny,
    fields: Picked<'a, TitleAndBody, 2>,
}

/// The keys of a dictionary of variants that a [`Picked`] keeps.
trait Keys<const N: usize> {
    const KEYS: [&'static str; N];
}

/// The `desktop-entry` hint of `Notify`.
struct DesktopEntry;

impl Keys<1> for DesktopEntry {
    const KEYS: [&'static str; 1] = ["desktop-entry"];
}

/// The `title` and `body` fields of `AddNotification`.
struct TitleAndBody;

impl Keys<2> for TitleAndBody {
    const KEYS: [&'static str; 2] = ["title", "body"];
}

/// The values of the keys `K` of a dictionary of variants, in the order of the keys.
///
/// The other values are skipped as they are read. A hint such as `image-data` holds a whole
/// picture, which a map of owned variants would hold as one variant per byte.
struct Picked<'a, K, const N: usize> {
    values: [Option<Variant<'a>>; N],
    keys: PhantomData<K>,
}

impl<K, const N: usize> Picked<'_, K, N> {
    /// The text at `index` of the keys, or an empty string when it is missing or isn't text.
    fn text(&self, index: usize) -> String {
        match &self.values[index] {
            Some(Variant::Str(text)) => text.as_str().to_owned(),
            _ => String::new(),
        }
    }
}

impl Picked<'_, DesktopEntry, 1> {
    fn desktop_entry(&self) -> String {
        self.text(0)
    }
}

impl Picked<'_, TitleAndBody, 2> {
    fn title(&self) -> String {
        self.text(0)
    }

    fn body(&self) -> String {
        self.text(1)
    }
}

impl<'de: 'a, 'a, K: Keys<N>, const N: usize> Deserialize<'de> for Picked<'a, K, N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(PickedVisitor(PhantomData))
    }
}

struct PickedVisitor<'a, K, const N: usize>(PhantomData<(&'a (), K)>);

impl<'de: 'a, 'a, K: Keys<N>, const N: usize> Visitor<'de> for PickedVisitor<'a, K, N> {
    type Value = Picked<'a, K, N>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a dictionary of variants")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut values = [const { None }; N];
        while let Some(key) = map.next_key::<&'a str>()? {
            match K::KEYS.iter().position(|wanted| *wanted == key) {
                Some(index) => values[index] = Some(map.next_value::<Variant<'a>>()?),
                None => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(Picked {
            values,
            keys: PhantomData,
        })
    }
}

fn parse(msg: &zbus::Message) -> Option<Notice> {
    let header = msg.header();
    if header.message_type() != message::Type::MethodCall {
        return None;
    }
    let body = msg.body();
    match (header.interface()?.as_str(), header.member()?.as_str()) {
        ("org.freedesktop.Notifications", "Notify") => {
            let notify: Notify<'_> = body.deserialize().ok()?;
            let desktop_entry = notify.hints.desktop_entry();
            let (app, app_id) = if notify.app.is_empty() {
                (desktop_entry, String::new())
            } else {
                (notify.app.to_owned(), desktop_entry)
            };
            Some(Notice {
                app,
                pid: None,
                app_id,
                title: notify.title.to_owned(),
                body: plain_text(notify.body),
            })
        }
        ("org.gtk.Notifications", "AddNotification") => {
            let added: AddNotification<'_> = body.deserialize().ok()?;
            Some(Notice {
                app: added.app.to_owned(),
                pid: None,
                app_id: String::new(),
                title: added.fields.title(),
                body: added.fields.body(),
            })
        }
        _ => None,
    }
}

/// The tags that the notification specification allows in a body.
const TAGS: [&str; 5] = ["b", "i", "u", "a", "img"];

/// Removes the markup that the notification specification allows from `body`.
///
/// A sender may send markup only to a service that supports it, so a `<` that doesn't start
/// an allowed tag stays as it is.
fn plain_text(body: &str) -> String {
    let mut text = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find(['<', '&']) {
        text.push_str(&rest[..at]);
        rest = &rest[at..];
        let skipped = if rest.starts_with('<') {
            tag_len(rest).map(|len| Markup { len, decoded: None })
        } else {
            entity(rest).map(|entity| Markup {
                len: entity.len,
                decoded: Some(entity.decoded),
            })
        };
        match skipped {
            Some(Markup { len, decoded }) => {
                text.extend(decoded);
                rest = &rest[len..];
            }
            None => {
                text.push_str(&rest[..1]);
                rest = &rest[1..];
            }
        }
    }
    text.push_str(rest);
    text
}

/// A piece of markup at the start of a text.
struct Markup {
    /// The length of the markup in bytes.
    len: usize,
    /// The character that the markup stands for, if any.
    decoded: Option<char>,
}

/// An entity at the start of a text.
struct Entity {
    /// The length of the entity in bytes.
    len: usize,
    decoded: char,
}

/// Returns the length of the allowed tag at the start of `text`.
fn tag_len(text: &str) -> Option<usize> {
    let inner = text.strip_prefix('<')?;
    let name = inner.strip_prefix('/').unwrap_or(inner);
    let name_len = name.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(name.len());
    if !TAGS.iter().any(|tag| tag.eq_ignore_ascii_case(&name[..name_len])) {
        return None;
    }
    let after = &name[name_len..];
    if !after.starts_with(['>', '/', ' ']) {
        return None;
    }
    Some(text.len() - after.len() + after.find('>')? + 1)
}

/// Decodes the entity at the start of `text`.
fn entity(text: &str) -> Option<Entity> {
    let end = text.get(..12).unwrap_or(text).find(';')?;
    let name = &text[1..end];
    let c = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)?
        }
    };
    Some(Entity {
        len: end + 1,
        decoded: c,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rstest::rstest;
    use zbus::wire::serialized::{Context, Data};
    use zbus::wire::{LE, to_bytes};

    use super::*;

    #[rstest]
    #[case::gnome_copy(":1.30", Some(":1.12"), Some(":1.30"), true)]
    #[case::application(":1.75", Some(":1.30"), Some(":1.30"), false)]
    #[case::service_to_its_name(":1.30", Some(SERVICE), Some(":1.30"), false)]
    #[case::service_to_itself(":1.30", Some(":1.30"), Some(":1.30"), false)]
    #[case::without_a_destination(":1.30", None, Some(":1.30"), false)]
    #[case::owner_unknown(":1.30", Some(":1.12"), None, false)]
    fn only_the_copy_that_the_service_passes_on_is_dropped(
        #[case] sender: &str,
        #[case] destination: Option<&str>,
        #[case] owner: Option<&str>,
        #[case] expected: bool,
    ) {
        assert_eq!(is_passed_on(sender, destination, owner), expected);
    }

    fn serialized<T: serde::Serialize + Type>(value: &T) -> Data<'static, 'static> {
        to_bytes(Context::new(LE, 0), value).expect("the value serializes")
    }

    /// The arguments of a `Notify` call with an empty application name and an image.
    #[rstest]
    fn notify_takes_the_desktop_entry_and_skips_the_image() {
        let image =
            Variant::from((64_i32, 64_i32, 256_i32, true, 8_i32, 4_i32, vec![7_u8; 16_384]));
        let hints: HashMap<&str, Variant<'_>> = HashMap::from([
            ("image-data", image),
            ("desktop-entry", Variant::from("org.example.Mail")),
        ]);
        let actions: Vec<&str> = Vec::new();
        let data = serialized(&("", 0_u32, "", "New mail", "<b>Hi</b>", actions, hints, -1_i32));
        let (notify, _) = data.deserialize::<Notify<'_>>().expect("the arguments deserialize");
        assert_eq!(notify.app, "");
        assert_eq!(notify.hints.desktop_entry(), "org.example.Mail");
        assert_eq!((notify.title, notify.body), ("New mail", "<b>Hi</b>"));
    }

    #[rstest]
    fn add_notification_takes_the_title_and_the_body() {
        let fields: HashMap<&str, Variant<'_>> = HashMap::from([
            ("title", Variant::from("Backup failed")),
            ("priority", Variant::from("high")),
            ("body", Variant::from("Disk full")),
        ]);
        let data = serialized(&("org.example.Backup", "id-1", fields));
        let (added, _) =
            data.deserialize::<AddNotification<'_>>().expect("the arguments deserialize");
        assert_eq!(added.app, "org.example.Backup");
        assert_eq!(
            (added.fields.title(), added.fields.body()),
            ("Backup failed".to_owned(), "Disk full".to_owned())
        );
    }

    #[rstest]
    #[case::plain("Build failed", "Build failed")]
    #[case::tags("<b>Build</b> failed in <i>main.rs</i>", "Build failed in main.rs")]
    #[case::link("see <a href=\"https://ci.example/1\">the log</a>", "see the log")]
    #[case::image("<img src=\"x.png\" alt=\"x\"/>done", "done")]
    #[case::entities("a &lt; b &amp;&amp; c &#62; d &#x41;", "a < b && c > d A")]
    #[case::stray_markup_characters("1 < 2 & 3 <br> &unknown;", "1 < 2 & 3 <br> &unknown;")]
    #[case::unclosed_tag("<b unclosed", "<b unclosed")]
    fn markup_becomes_plain_text(#[case] body: &str, #[case] expected: &str) {
        assert_eq!(plain_text(body), expected);
    }
}
