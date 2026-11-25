//! Hands out the notifications that the platform delivers apart from windows.

use std::time::{Duration, SystemTime};

use tracing::debug;

use super::context::Context;
use super::record::{Content, EncodedTree, Notification, Record};
use super::scrubber::Found;
use crate::backend::{
    AppIdentity, Backend, Completeness, Node, Notice, ProcessApp, ReadStats, Role,
};

/// What the tree of a notification counts as having cost. The text of a notification arrives
/// with the notification, so nothing is read.
const NOTICE_READ: ReadStats = ReadStats {
    nodes: 2,
    completeness: Completeness::Complete,
    unknown_roles: 0,
};

/// The element tree of a notification: a window with one paragraph that holds `body`.
///
/// [`Content::of_notice`] builds the content of a notification with this tree.
#[must_use]
fn notice_tree(body: String) -> Node {
    Node {
        role: Role::Window,
        children: vec![Node {
            role: Role::Paragraph,
            text: Some(body),
            ..Node::default()
        }],
        ..Node::default()
    }
}

impl Found {
    /// The tree of a notification whose text is `body`, as capture reads it.
    fn of_notice(body: String) -> Self {
        Self::new(notice_tree(body), Duration::ZERO, NOTICE_READ)
    }
}

impl Content {
    /// The content of a notification whose text is `body`, in the form that capture hands
    /// out. `body` goes in as it is, so it must already be clean.
    #[must_use]
    pub fn of_notice(body: String) -> Self {
        Self {
            tree: EncodedTree::encode(&notice_tree(body)),
            read_time: Duration::ZERO,
            read: NOTICE_READ,
        }
    }
}

/// Hands out `notice`, unless its application is filtered.
///
/// A notice that doesn't name its application takes the name of the application that runs
/// as its process. A notice whose application stays unknown is handed out only when the
/// filter allows every application.
pub async fn notify<B: Backend>(ctx: &mut Context<B>, notice: Notice) {
    let Notice {
        app,
        pid,
        app_id,
        title,
        body,
    } = notice;
    let Some(app) = kept_name(ctx, Declared { app, app_id }, pid).await else {
        return;
    };
    let content = ctx.scrubber.clean(Found::of_notice(body)).into_content();
    ctx.records.push_back(Record::Notification(Notification {
        app,
        title: ctx.scrubber.title(title),
        at: SystemTime::now(),
        content,
    }));
}

/// The application that a notice declares.
struct Declared {
    /// The name of the application, which is empty when the notice doesn't name one.
    app: String,
    /// The app id that the notice names besides `app`, which is empty when the notice doesn't
    /// name one.
    app_id: String,
}

/// The name of the application under which capture keeps a notice, or `None` when the filter
/// leaves the notice out.
///
/// The notice declares `declared`, and the process `pid` sent the notice. The name is empty for a
/// notice whose application is unknown.
async fn kept_name<B: Backend>(
    ctx: &mut Context<B>,
    declared: Declared,
    pid: Option<u32>,
) -> Option<String> {
    let Declared { app, app_id } = declared;
    if !app.is_empty() {
        if !allows_declared(ctx, &app, &app_id).await {
            debug!(app, app_id, "ignoring a notification of a filtered application");
            return None;
        }
        // The sender chooses the name that a notification declares, so the process behind the
        // notification is checked too.
        if let Some(pid) = pid
            && !ctx.allows_sender(pid).await
        {
            debug!(app, pid, "ignoring a notification from a filtered application");
            return None;
        }
        return Some(ctx.scrubber.app_name(app));
    }
    if let Some(pid) = pid
        && let ProcessApp::App(sender) = ctx.app_of_sender(pid).await
    {
        let name = ctx.allowed_app_name(&sender).await.map(str::to_owned);
        if name.is_none() {
            debug!(pid, "ignoring a notification from a filtered application");
        }
        return name;
    }
    if !ctx.config.filter.allows_every_app() {
        debug!("ignoring a notification of an unknown application");
        return None;
    }
    Some(app)
}

/// Whether capture may keep a notice that declares the application name `app` and the app id
/// `app_id`.
///
/// The app id, and a name that reads as an app id, stand for the names in the application's
/// desktop entry and for the running applications of the id. For example, a Flatpak
/// application's notifications reach GNOME Shell under the application's id, such as
/// `org.gnome.Fractal`, and users exclude the application by its name. The notice passes only
/// when capture may read an application that goes by every one of these names.
async fn allows_declared<B: Backend>(ctx: &mut Context<B>, app: &str, app_id: &str) -> bool {
    let mut names = Vec::new();
    let other_id = (app != app_id).then_some(app);
    for id in std::iter::once(app_id).chain(other_id).filter(|id| reads_as_app_id(id)) {
        let AppIdentity { entry_names, apps } = ctx.backend.identify_app_id(id).await;
        names.extend(entry_names);
        for running in apps {
            if ctx.backend.is_left_out(&running) {
                return false;
            }
            let Ok(name) = ctx.app_name(&running).await.map(str::to_owned) else {
                continue;
            };
            if ctx.backend.is_terminal(&running, &name) {
                return false;
            }
            names.push(name);
        }
    }
    ctx.allows_app_named(std::iter::once(app).chain(names.iter().map(String::as_str)))
}

/// Whether `name` reads as an app id: names joined by dots in reverse-DNS order, such as
/// `org.gnome.Fractal`.
fn reads_as_app_id(name: &str) -> bool {
    let valid = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    name.contains('.') && name.split('.').all(valid)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::flatpak("org.gnome.Fractal", true)]
    #[case::desktop_entry("org.telegram.desktop", true)]
    #[case::hyphen_and_underscore("de.schmidhuberj.Flare-beta_2", true)]
    #[case::plain_name("Firefox", false)]
    #[case::version("Firefox 3.5", false)]
    #[case::empty_part("org..Fractal", false)]
    #[case::trailing_dot("Fractal.", false)]
    #[case::path("../org.gnome.Fractal", false)]
    #[case::empty("", false)]
    fn names_that_read_as_app_ids(#[case] name: &str, #[case] expected: bool) {
        assert_eq!(reads_as_app_id(name), expected);
    }
}
