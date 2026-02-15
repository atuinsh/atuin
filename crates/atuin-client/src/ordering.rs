use minspan::minspan;

use super::{database::Context, history::History, settings::SearchMode};

pub fn reorder_fuzzy(mode: SearchMode, query: &str, res: Vec<History>) -> Vec<History> {
    match mode {
        SearchMode::Fuzzy => reorder(query, |x| &x.command, res),
        _ => res,
    }
}

fn reorder<F, A>(query: &str, f: F, res: Vec<A>) -> Vec<A>
where
    F: Fn(&A) -> &String,
    A: Clone,
{
    let mut r = res.clone();
    let qvec = &query.chars().collect();
    r.sort_by_cached_key(|h| {
        // TODO for fzf search we should sum up scores for each matched term
        let (from, to) = match minspan::span(qvec, &(f(h).chars().collect())) {
            Some(x) => x,
            // this is a little unfortunate: when we are asked to match a query that is found nowhere,
            // we don't want to return a None, as the comparison behaviour would put the worst matches
            // at the front. therefore, we'll return a set of indices that are one larger than the longest
            // possible legitimate match. This is meaningless except as a comparison.
            None => (0, res.len()),
        };
        1 + to - from
    });
    r
}

/// Scope priority levels from narrowest (most specific) to broadest.
/// Lower values = higher priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ScopePriority {
    Session = 0,   // Current shell session
    Directory = 1, // Current working directory
    Host = 2,      // Current machine
    Global = 3,    // Everything else
}

/// Determine the scope priority for a history entry based on current context.
fn get_scope_priority(history: &History, context: &Context) -> ScopePriority {
    if history.session == context.session {
        ScopePriority::Session
    } else if history.cwd == context.cwd {
        ScopePriority::Directory
    } else if history.hostname == context.hostname {
        ScopePriority::Host
    } else {
        ScopePriority::Global
    }
}

/// Reorder results by scope priority: session → directory → host → global.
/// Within each scope tier, the original order (typically by timestamp desc) is preserved.
pub fn reorder_by_scope_priority(context: &Context, res: Vec<History>) -> Vec<History> {
    // Group results by scope priority while preserving original order within each tier
    let mut session: Vec<History> = Vec::new();
    let mut directory: Vec<History> = Vec::new();
    let mut host: Vec<History> = Vec::new();
    let mut global: Vec<History> = Vec::new();

    for h in res {
        match get_scope_priority(&h, context) {
            ScopePriority::Session => session.push(h),
            ScopePriority::Directory => directory.push(h),
            ScopePriority::Host => host.push(h),
            ScopePriority::Global => global.push(h),
        }
    }

    // Combine in priority order
    let mut result = session;
    result.extend(directory);
    result.extend(host);
    result.extend(global);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    fn make_history_full(command: &str, session: &str, cwd: &str, hostname: &str) -> History {
        History::import()
            .command(command.to_string())
            .session(session.to_string())
            .cwd(cwd.to_string())
            .hostname(hostname.to_string())
            .timestamp(OffsetDateTime::now_utc())
            .build()
            .into()
    }

    fn make_context() -> Context {
        Context {
            session: "current-session".to_string(),
            cwd: "/home/user/project".to_string(),
            hostname: "my-host".to_string(),
            host_id: "host-id-123".to_string(),
            git_root: None,
        }
    }

    #[test]
    fn test_scope_priority_orders_by_narrowest_first() {
        let context = make_context();
        let results = vec![
            make_history_full("global-cmd", "other-session", "/other/dir", "other-host"),
            make_history_full("session-cmd", "current-session", "/other/dir", "other-host"),
            make_history_full("host-cmd", "other-session", "/other/dir", "my-host"),
            make_history_full(
                "dir-cmd",
                "other-session",
                "/home/user/project",
                "other-host",
            ),
        ];

        let ordered = reorder_by_scope_priority(&context, results);

        // Session first, then directory, then host, then global
        assert_eq!(ordered[0].command, "session-cmd");
        assert_eq!(ordered[1].command, "dir-cmd");
        assert_eq!(ordered[2].command, "host-cmd");
        assert_eq!(ordered[3].command, "global-cmd");
    }

    #[test]
    fn test_scope_priority_preserves_order_within_tier() {
        let context = make_context();
        let results = vec![
            make_history_full("cmd1", "current-session", "/x", "x"),
            make_history_full("cmd2", "current-session", "/y", "y"),
            make_history_full("cmd3", "current-session", "/z", "z"),
        ];

        let ordered = reorder_by_scope_priority(&context, results);

        // All are session scope, so original order preserved
        assert_eq!(ordered[0].command, "cmd1");
        assert_eq!(ordered[1].command, "cmd2");
        assert_eq!(ordered[2].command, "cmd3");
    }

    #[test]
    fn test_scope_priority_with_mixed_tiers() {
        let context = make_context();
        let results = vec![
            make_history_full("g1", "x", "/x", "x"), // global
            make_history_full("s1", "current-session", "/x", "x"), // session
            make_history_full("h1", "x", "/x", "my-host"), // host
            make_history_full("d1", "x", "/home/user/project", "x"), // directory
            make_history_full("g2", "y", "/y", "y"), // global
            make_history_full("s2", "current-session", "/y", "y"), // session
        ];

        let ordered = reorder_by_scope_priority(&context, results);

        // Sessions first (in order), then directory, then host, then globals (in order)
        assert_eq!(ordered[0].command, "s1");
        assert_eq!(ordered[1].command, "s2");
        assert_eq!(ordered[2].command, "d1");
        assert_eq!(ordered[3].command, "h1");
        assert_eq!(ordered[4].command, "g1");
        assert_eq!(ordered[5].command, "g2");
    }
}
