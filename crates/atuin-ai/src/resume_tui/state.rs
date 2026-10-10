//! The picker's state: input, filter mode, results, and key handling.
//!
//! Mirrors the history search's `State` (resolve a key to an action, then execute it), minus the
//! parts that only make sense for history (search modes, contexts, deletion).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use atuin_client::ai_session::{HarnessKind, HarnessSession};
use atuin_client::settings::{AiSessionFilterMode as FilterMode, KeymapMode, Settings};
use atuin_client::theme::Meaning;
use atuin_client::tui::cursor::Cursor;
use atuin_client::tui::key::{KeyCodeValue, KeyInput, SingleKey};
use atuin_common::harnesstools::continuation::Flattened;
use atuin_common::time::OffsetDateTimeExt as _;
use atuin_domain::record::HostId;
use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use time::OffsetDateTime;

use super::chooser::{Chooser, Destination};
use super::keymap::{Action, Keymap, KeymapSet};
use super::query::{self, ParsedQuery};
use super::reader::Rendered;
use super::rebuild::Rebuilding;
use super::resumer::{NotResumable, Resume};
use super::source::{SessionFilter, SessionPreview, SessionRow, Transcript};
use super::{ResumeContext, panel};

/// Sessions updated this recently are live: a dot in the row, and refreshed while open.
pub const LIVE_SECS: u64 = 120;

/// What [`State::requested`] tracks per session.
pub const PREVIEW: u8 = 0;
pub const CHILDREN: u8 = 1;
pub const PLAN: u8 = 2;
/// Restoring the session's transcript from sync, or catching its copy here up with sync, once an
/// action is waiting on it.
pub const RESTORE: u8 = 3;
pub const TRANSCRIPT: u8 = 4;

/// The status while an empty workspace falls back to every session.
const FELL_BACK: &str = "no sessions in this repo yet: showing all of them";

/// How many sessions' transcripts are kept read: the reader shows one at a time, and a long
/// session's is large.
const TRANSCRIPTS_KEPT: usize = 16;

/// How many rows a search asks for.
pub const SEARCH_LIMIT: usize = 500;

/// How long the preview keeps showing the session it showed after the selection moves to one
/// whose preview isn't read yet: long enough to cover the read (and a held arrow key), so the
/// preview never blanks between two sessions, but short enough never to pass for the new one's.
pub const HOLD: Duration = Duration::from_millis(300);

/// How often the reader's spinner turns.
pub const SPIN: Duration = Duration::from_millis(80);

/// How many lines a turn of the mouse wheel scrolls a preview.
pub const WHEEL_LINES: usize = 3;

/// The panes whose text scrolls: the preview strip under the list, the detail pane beside it on
/// wide terminals, and Inspect's conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Strip = 0,
    Side = 1,
    Inspect = 2,
}

const PANES: [Pane; 3] = [Pane::Strip, Pane::Side, Pane::Inspect];

/// A pane's scroll position, and where and how it was drawn last.
///
/// The position is the session's: moving to another session starts it at the top again. At the
/// top, a pane shows its overview (the first prompt, match and last reply sharing the lines);
/// scrolled, it shows the parts in full, one after another.
#[derive(Debug, Default, Clone)]
pub struct PaneScroll {
    /// The session scrolled.
    pub session: Option<HarnessSession>,
    /// The first line shown of the parts in full; 0 is the overview.
    pub offset: usize,
    /// Where the pane was drawn this frame (screen coordinates, so an inline viewport's rows are
    /// as the mouse reports them), or `None` when it wasn't.
    pub area: Option<Rect>,
    /// How many lines of text it had room for.
    pub height: usize,
    /// The lines of the parts in full rendered so far.
    pub len: usize,
    /// More lines than `len`, not rendered yet (they are as it scrolls down).
    pub more: bool,
    /// The session the reader showed here last, and for which query (see [`super::reader`]): the
    /// reader opens at the search's match when either changes.
    pub reading: Option<(HarnessSession, String)>,
}

impl PaneScroll {
    /// Where `session`'s text starts: 0 for a session other than the one scrolled.
    pub fn offset_for(&self, session: &HarnessSession) -> usize {
        if self.session.as_ref() == Some(session) {
            self.offset
        } else {
            0
        }
    }

    /// The furthest the pane can scroll, as far as it was rendered: past the end is let through
    /// while there's more to render.
    pub fn max_offset(&self) -> usize {
        if self.more {
            self.len
        } else {
            self.len.saturating_sub(self.height)
        }
    }

    /// Scroll by `by` lines (up when negative), within the text.
    fn scroll(&mut self, by: isize) {
        let offset = self.offset.saturating_add_signed(by);
        self.offset = offset.min(self.max_offset());
    }
}

/// What the event loop should do after an input event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    Continue,
    Redraw,
    /// Resume the session acted on ([`State::target`]) now.
    Resume,
    /// Put the session's resume command on the command line.
    ReturnCommand,
    /// Copy the session's resume command, and stay open.
    Copy,
    /// A line of the chooser picked.
    Pick(Box<Picked>),
    /// Ask where to resume the session acted on, the fork selected.
    Fork,
    ReturnOriginal,
    Exit,
}

/// A line of the chooser, picked: the session it opened on (not whatever the list has selected
/// since: an idle refresh may have moved it), what the line does with it, then what the key
/// asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    pub row: SessionRow,
    pub line: Destination,
    pub action: Pending,
}

/// A continuation (or a fork, or a switch) asked of the worker, waiting to be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Continuing {
    /// Which request it is: only its own answer finishes it.
    pub id: u64,
    pub target: HarnessKind,
    pub kind: Writing,
    pub action: Pending,
}

/// What a [`Continuing`] writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writing {
    /// A new session of another harness.
    Continuation,
    /// A fork, in the session's own harness.
    Fork,
    /// The copy here, switched to another branch in place.
    Switch,
}

/// An action waiting for the selected session's resume plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    Resume,
    Edit,
    Copy,
}

/// Inspect's list of the forks grouped under the session inspected, once expanded (`c`): it has
/// the arrow keys, with a cursor, and scrolls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildrenView {
    /// The session whose children these are: selecting another collapses the list.
    pub session: HarnessSession,
    pub cursor: usize,
    /// The first row shown.
    pub offset: usize,
    /// How many rows showed last time it was drawn (a page).
    pub height: usize,
}

/// A session's transcript, read for the reader.
#[derive(Debug, Clone)]
pub struct Read {
    pub transcript: Arc<Transcript>,
    /// How many messages the session had when it was read: a refresh finding more reads it again.
    pub messages: u64,
    /// When it was read, among the others (for keeping the last few).
    pub order: u64,
    /// For the query last asked of it: whether no text in it holds the query (the match is in a
    /// session grouped under it, or a tool call's input).
    pub elsewhere: Option<(String, bool)>,
}

/// The selection and scroll position of the session list.
#[derive(Debug, Default)]
pub struct ListState {
    /// The first line shown, counted from the input outward (see [`super::render::list_lines`]).
    pub offset: usize,
    pub selected: usize,
    /// How many rows fit at once: what a page moves.
    pub max_entries: usize,
    /// The lines the list has, headings included.
    pub lines: usize,
    /// Whether the list drew the chooser under its row last frame: if not, it's a popup.
    pub chooser_drawn: bool,
    /// How far the list moved to keep the row's title in place when the chooser opened under it:
    /// moved back when it closes. `None` while it isn't open in the list.
    pub chooser_shift: Option<usize>,
}

#[allow(clippy::struct_excessive_bools)]
pub struct State {
    pub input: Cursor,
    pub keymap_mode: KeymapMode,
    keymaps: KeymapSet,
    pending_vim_key: Option<char>,
    pub tab_index: usize,
    pub list: ListState,
    pub results: Vec<SessionRow>,

    pub context: ResumeContext,
    pub mode: FilterMode,
    /// Open on every session instead when the workspace has none: only for the first results,
    /// and only until the user picks a mode with ctrl-r.
    workspace_fallback: bool,

    /// The generation of the newest search sent to the worker.
    pub issued: u64,
    /// The generation whose results are on screen.
    pub applied: u64,
    last_filter: Option<SessionFilter>,
    /// Whether the query of the last search sent had chips in it (the scope's own filters, such
    /// as the branch mode's branch, aren't chips: clearing the search doesn't lift them).
    last_chips: bool,
    /// The generation of a refresh in flight, whose results keep the selection.
    refreshing: Option<u64>,

    pub previews: HashMap<HarnessSession, SessionPreview>,
    /// Previews read again on a refresh (their sessions are live): shown as they are until the
    /// new ones come.
    pub stale: HashSet<HarnessSession>,
    /// The reader's conversations, read (see [`Read`]).
    pub transcripts: HashMap<HarnessSession, Read>,
    /// Transcripts read again because their sessions grew, shown as they are meanwhile.
    pub stale_transcripts: HashSet<HarnessSession>,
    /// Sessions whose transcripts couldn't be read: their panes show the preview instead.
    pub unreadable: HashSet<HarnessSession>,
    /// Whether the selection is moving faster than [`super::SETTLE`] (a held arrow key): details
    /// wait for it, and the reader shows a spinner.
    pub settling: bool,
    /// Whether the last frame drew the reader's spinner, which then needs drawing again.
    pub spinning: bool,
    /// When the picker opened: the spinner turns from then, unless motion is reduced (`None`).
    pub spin_from: Option<Instant>,
    /// How many transcripts have been read, for [`Read::order`].
    transcripts_read: u64,
    /// Whether the last frame drew a reader (the pane beside the list, or Inspect): transcripts
    /// are only read for one.
    pub reader_visible: bool,
    /// Each pane's reader's rendered lines, kept between frames, by [`Pane`].
    pub readers: [Option<Rendered>; 3],
    /// The query text of the results on screen (not the input, which may have moved on): what
    /// the list groups by and the reader opens at.
    pub applied_query: String,
    /// Whether the results on screen are narrowed by chips (`b:`, `m:`, `h:`) as well.
    pub applied_chips: bool,
    /// The session whose preview was shown last, and when a frame was first drawn with another
    /// selected: the preview keeps showing it for [`HOLD`] after the selection moves to one not
    /// read yet.
    pub shown: Option<(SessionRow, Option<Instant>)>,
    /// The tallest the preview strip has been (with `preview.strategy = "auto"`): it doesn't
    /// shrink again, so the list doesn't jump as the selection moves.
    pub strip_height: u16,
    /// Each scrolling pane's position, by [`Pane`].
    pub scrolls: [PaneScroll; 3],
    /// The forks grouped under each session, once read (see
    /// [`super::source::SessionSource::children`]).
    pub children: HashMap<HarnessSession, Vec<SessionRow>>,
    /// Inspect's list of forks, while it's expanded.
    pub children_view: Option<ChildrenView>,
    /// Details asked of the worker and not answered yet. The worker drops a request superseded by
    /// a newer one of its kind, so only the selected session's entries, and those of the session
    /// an action waits on, are kept (see [`Self::forget_unanswered`]).
    pub requested: HashSet<(HarnessSession, u8)>,
    /// Sessions named by id, kept first while the query is still the id they were named by: one
    /// that can't be resumed, so the picker opens on it and says why, or the several an id
    /// names, to pick one.
    pinned: Option<(Vec<SessionRow>, String)>,
    /// Resume plans, fetched for the selected session only (planning may walk directories). A
    /// plan to restore the session from sync is replaced by the plain one once it is restored.
    pub plans: HashMap<HarnessSession, Result<Resume, NotResumable>>,
    /// An enter/tab/ctrl-y waiting for its session's plan (or restore), and the session it acts
    /// on: that row, not whatever the list shows by the time the answer comes, which an idle
    /// refresh may have moved or dropped meanwhile.
    pub pending: Option<(SessionRow, Pending)>,
    /// The "Resume in" chooser, while it's open.
    pub chooser: Option<Chooser>,
    /// What continuing each session elsewhere would flatten, once read (see
    /// [`Request::Flatten`](super::worker::Request::Flatten)); `Err` when it can't be read.
    pub flattened: HashMap<HarnessSession, Result<Flattened, String>>,
    /// The last flattening asked of the worker (which keeps only the newest).
    pub flattening: Option<HarnessSession>,
    /// A continuation being written, and what to do once it is.
    pub continuing: Option<Continuing>,
    /// The last continuation's id.
    pub continued: u64,
    /// What catching the session being resumed up with sync wrote, to say once the picker is gone.
    pub note: Option<String>,

    /// A one-line message in the status row (copied, can't resume, search failed).
    pub status: Option<(String, Meaning)>,
    /// The daemon is rebuilding the session index, so results may be incomplete: said in the
    /// status row while it has nothing else to say.
    pub rebuilding: Option<Rebuilding>,
    pub now: Box<dyn Fn() -> OffsetDateTime + Send>,
}

impl State {
    pub fn new(settings: &Settings, context: ResumeContext, query: &str) -> Self {
        let sessions = &settings.ai.sessions;
        let mut input = Cursor::from(query.to_owned());
        input.end();

        let mut state = Self {
            input,
            keymap_mode: match settings.keymap_mode {
                KeymapMode::Auto => KeymapMode::Emacs,
                mode => mode,
            },
            keymaps: KeymapSet::defaults(settings),
            pending_vim_key: None,
            tab_index: 0,
            list: ListState::default(),
            results: Vec::new(),
            context,
            mode: FilterMode::Global,
            workspace_fallback: false,
            issued: 0,
            applied: 0,
            last_filter: None,
            last_chips: false,
            refreshing: None,
            previews: HashMap::new(),
            stale: HashSet::new(),
            transcripts: HashMap::new(),
            stale_transcripts: HashSet::new(),
            unreadable: HashSet::new(),
            settling: false,
            spinning: false,
            spin_from: (!settings.prefers_reduced_motion).then(Instant::now),
            transcripts_read: 0,
            reader_visible: false,
            readers: Default::default(),
            applied_query: String::new(),
            applied_chips: false,
            shown: None,
            strip_height: 0,
            scrolls: Default::default(),
            children: HashMap::new(),
            children_view: None,
            requested: HashSet::new(),
            pinned: None,
            plans: HashMap::new(),
            pending: None,
            chooser: None,
            flattened: HashMap::new(),
            flattening: None,
            continuing: None,
            continued: 0,
            note: None,
            status: None,
            rebuilding: None,
            now: if settings.prefers_reduced_motion {
                let now = OffsetDateTime::now_utc();
                Box::new(move || now)
            } else {
                Box::new(OffsetDateTime::now_utc)
            },
        };
        state.set_initial_mode(sessions.filter_mode);
        state
    }

    /// Pick the opening filter: the configured one if it can apply here; else, for branch on a
    /// detached `HEAD`, the repository's; else every session's.
    fn set_initial_mode(&mut self, configured: Option<FilterMode>) {
        let fallback = if self.mode_available(FilterMode::Workspace) {
            FilterMode::Workspace
        } else {
            FilterMode::Global
        };
        self.mode = match configured {
            Some(mode) if self.mode_available(mode) => mode,
            Some(_) => fallback,
            None => FilterMode::Global,
        };
        self.workspace_fallback = self.mode == FilterMode::Workspace;
    }

    pub fn mode_available(&self, mode: FilterMode) -> bool {
        match mode {
            FilterMode::Workspace => self.context.git_root.is_some(),
            FilterMode::Branch => self.context.git_root.is_some() && self.context.branch.is_some(),
            FilterMode::Global | FilterMode::Host | FilterMode::Directory => true,
        }
    }

    /// The mode ctrl-r goes to next: the next available one in [`FilterMode::CYCLE`], wrapping
    /// around. `None` when there is no other.
    pub fn next_mode(&self) -> Option<FilterMode> {
        let modes = FilterMode::CYCLE;
        let at = modes.iter().position(|m| *m == self.mode).unwrap_or(modes.len() - 1);
        (1..=modes.len())
            .map(|step| modes[(at + step) % modes.len()])
            .find(|mode| self.mode_available(*mode))
            .filter(|mode| *mode != self.mode)
    }

    /// ctrl-r: the next available mode in [`FilterMode::CYCLE`].
    pub fn cycle_filter_mode(&mut self) {
        if let Some(mode) = self.next_mode() {
            self.mode = mode;
        }
        self.workspace_fallback = false;
        // What the fallback said no longer holds once the scope moves.
        if self.status.as_ref().is_some_and(|(s, _)| s == FELL_BACK) {
            self.status = None;
        }
    }

    pub fn parsed_query(&self) -> ParsedQuery {
        query::parse(self.input.as_str())
    }

    /// The source filter for the current mode and query. Forks are always grouped under their
    /// roots.
    pub fn filter(&self) -> SessionFilter {
        let q = self.parsed_query();
        let ctx = &self.context;
        let mut filter = SessionFilter {
            text: q.text,
            limit: SEARCH_LIMIT,
            ..SessionFilter::default()
        };
        let db = &mut filter.db;
        db.harness = q.harness;
        db.model = q.model;
        db.branch = q.branch;
        db.roots_only = true;
        match self.mode {
            FilterMode::Global => {}
            FilterMode::Host => {
                // This host's sessions from before hosts were recorded are its own. An id that
                // isn't a UUID names no recorded host: only those.
                let id = uuid::Uuid::try_parse(&ctx.host_id).unwrap_or_default();
                db.host = Some(HostId(id));
                db.or_unrecorded = true;
            }
            FilterMode::Workspace => db.workspace.clone_from(&ctx.git_root),
            FilterMode::Directory => db.directory = Some(ctx.cwd.clone()),
            FilterMode::Branch => {
                db.workspace.clone_from(&ctx.git_root);
                if db.branch.is_none() {
                    db.branch.clone_from(&ctx.branch);
                }
            }
        }
        filter
    }

    /// The next search to run, if the filter changed since the last one sent. Bumps the
    /// generation, so results for anything older are dropped when they arrive.
    pub fn next_search(&mut self) -> Option<(u64, SessionFilter)> {
        let filter = self.filter();
        if self.last_filter.as_ref() == Some(&filter) {
            return None;
        }
        self.last_filter = Some(filter.clone());
        let q = self.parsed_query();
        self.last_chips = q.harness.is_some() || q.model.is_some() || q.branch.is_some();
        self.issued += 1;
        Some((self.issued, filter))
    }

    /// Apply a search's results. Stale generations are dropped (the list on screen stays until
    /// the newest search answers). `true` when they were applied, or when an empty workspace fell
    /// back to every session (to be searched next).
    pub fn apply_results(&mut self, generation: u64, mut rows: Vec<SessionRow>) -> bool {
        if generation != self.issued {
            return false;
        }
        if let Some((pinned, query)) = &self.pinned {
            if self.input.as_str() == query {
                rows.retain(|r| pinned.iter().all(|p| p.handle != r.handle));
                rows.splice(0..0, pinned.iter().cloned());
            } else {
                self.pinned = None;
                self.status = None;
            }
        }
        // These results are for the newest filter sent.
        let text = self.last_filter.as_ref().map(|f| f.text.trim().to_owned()).unwrap_or_default();
        // Only the opening results say whether the workspace has sessions at all, and only with
        // nothing in the query (text, or a chip) to narrow them.
        let chips = self.last_chips;
        let unnarrowed = self.last_filter.is_some() && text.is_empty() && !chips;
        if std::mem::take(&mut self.workspace_fallback) && rows.is_empty() && unnarrowed {
            self.mode = FilterMode::Global;
            self.status = Some((FELL_BACK.to_owned(), Meaning::Annotation));
            return true;
        }
        let refreshed = self.refreshing == Some(generation);
        if refreshed {
            self.stale_grown(&rows);
        }
        let selected = self.selected().map(|r| r.handle.clone());
        self.results = rows;
        self.applied = generation;
        self.applied_query = text;
        self.applied_chips = chips;
        // A refresh keeps the selection on the same session; a new query starts at the best
        // match, as the history search does.
        let keep = self.refreshing.take() == Some(generation);
        let index = selected
            .filter(|_| keep)
            .and_then(|h| self.results.iter().position(|r| r.handle == h))
            .unwrap_or(0);
        self.list.selected = index;
        true
    }

    /// Search again with the same filter, so live sessions move and their previews catch up.
    /// Skipped while a search is still out.
    pub fn refresh(&mut self) -> Option<(u64, SessionFilter)> {
        if self.issued != self.applied {
            return None;
        }
        self.last_filter = None;
        let next = self.next_search()?;
        self.refreshing = Some(next.0);
        let now = (self.now)();
        let live: Vec<HarnessSession> = self
            .results
            .iter()
            .filter(|r| now.saturating_duration_since(r.updated_at).as_secs() < LIVE_SECS)
            .map(|r| r.handle.clone())
            .collect();
        // Read again, and shown as they are meanwhile: dropping them blanked the preview (and
        // shrank it, moving the list) until the new ones came.
        for handle in live {
            self.requested.remove(&(handle.clone(), PREVIEW));
            if self.previews.contains_key(&handle) {
                self.stale.insert(handle);
            }
        }
        Some(next)
    }

    /// Whether `session`'s preview is to be read: it isn't yet, or a refresh wants it again.
    pub fn wants_preview(&self, session: &HarnessSession) -> bool {
        !self.previews.contains_key(session) || self.stale.contains(session)
    }

    /// A preview read.
    pub fn apply_preview(&mut self, session: HarnessSession, preview: SessionPreview) {
        self.stale.remove(&session);
        self.previews.insert(session, preview);
    }

    /// Whether `session`'s transcript is to be read: a reader is showing, and it isn't read yet,
    /// or it has grown since.
    /// Whether `row`'s conversation is on its way to the reader: the selection is still moving,
    /// or its transcript is being read.
    pub fn reading(&self, row: &SessionRow) -> bool {
        self.settling
            || !(self.transcripts.contains_key(&row.handle)
                || self.unreadable.contains(&row.handle))
    }

    pub fn wants_transcript(&self, session: &HarnessSession) -> bool {
        self.reader_visible
            && (!self.transcripts.contains_key(session) || self.stale_transcripts.contains(session))
    }

    /// Mark for reading again the transcripts of sessions a refresh found with more messages
    /// than they were read with: a live session that went quiet isn't read over and over.
    fn stale_grown(&mut self, rows: &[SessionRow]) {
        for row in rows {
            if self.transcripts.get(&row.handle).is_some_and(|t| t.messages != row.messages) {
                self.requested.remove(&(row.handle.clone(), TRANSCRIPT));
                self.stale_transcripts.insert(row.handle.clone());
            }
        }
    }

    /// A transcript read, or why it couldn't be. A failed read keeps what was read before, if
    /// anything (the session's preview stands in otherwise); it isn't asked for again until the
    /// selection comes back to it. At most [`TRANSCRIPTS_KEPT`] are kept: the one read longest
    /// ago (never the selected session's) gives way. Read again unchanged, it keeps what the
    /// reader rendered of it, and the count it was read at moves on.
    pub fn apply_transcript(&mut self, session: HarnessSession, read: Result<Transcript, String>) {
        self.stale_transcripts.remove(&session);
        let Ok(transcript) = read else {
            // Unless one was read before, shown as it is.
            self.unreadable.insert(session);
            return;
        };
        self.unreadable.remove(&session);
        if let Some(t) = self.transcripts.get_mut(&session)
            && t.transcript.entries == transcript.entries
        {
            t.messages = transcript.message_count;
            return;
        }
        if !self.transcripts.contains_key(&session) && self.transcripts.len() >= TRANSCRIPTS_KEPT {
            let selected = self.selected().map(|r| r.handle.clone());
            let oldest = self
                .transcripts
                .iter()
                .filter(|(s, _)| Some(*s) != selected.as_ref())
                .min_by_key(|(_, t)| t.order)
                .map(|(s, _)| s.clone());
            if let Some(oldest) = oldest {
                self.transcripts.remove(&oldest);
            }
        }
        self.transcripts_read += 1;
        self.transcripts.insert(session, Read {
            elsewhere: None,
            messages: transcript.message_count,
            transcript: Arc::new(transcript),
            order: self.transcripts_read,
        });
    }

    /// The session the preview shows at `now`: the selected one once its preview is read. Until
    /// then, the one it showed before, for up to [`HOLD`] after the selection left it, rather
    /// than nothing; past that, the selected one, waiting (`…`).
    pub fn preview_row_at(&self, now: Instant) -> Option<&SessionRow> {
        let selected = self.selected()?;
        if self.previews.contains_key(&selected.handle) {
            return Some(selected);
        }
        match &self.shown {
            Some((row, left))
                if left.is_none_or(|at| now.saturating_duration_since(at) < HOLD)
                    && self.previews.contains_key(&row.handle) =>
            {
                Some(row)
            }
            _ => Some(selected),
        }
    }

    /// The session the preview shows now (see [`Self::preview_row_at`]).
    pub fn preview_row(&self) -> Option<&SessionRow> {
        self.preview_row_at(Instant::now())
    }

    /// Note a frame drawn at `now`: while the selected session's preview is read, it is the one
    /// to hold when the selection moves on; once it has, the hold runs from the first frame
    /// drawn without it.
    pub fn preview_drawn(&mut self, now: Instant) {
        let Some(selected) = self.selected().cloned() else {
            return;
        };
        if self.previews.contains_key(&selected.handle) {
            // The row as it is now: a refresh may have changed it.
            self.shown = Some((selected, None));
        } else if let Some((row, left @ None)) = &mut self.shown
            && row.handle != selected.handle
        {
            *left = Some(now);
        }
    }

    /// When a preview held for a selection not read yet gives way to it (`…`): the picker draws
    /// again then.
    pub fn hold_ends(&self) -> Option<Instant> {
        let selected = self.selected()?;
        if self.previews.contains_key(&selected.handle) {
            return None;
        }
        let (row, left) = self.shown.as_ref()?;
        let ends = (*left)? + HOLD;
        (row.handle != selected.handle && ends > Instant::now()).then_some(ends)
    }

    /// The scrolling pane drawn last: the one the preview keys move.
    fn drawn_pane(&self) -> Option<Pane> {
        PANES.into_iter().find(|p| self.scrolls[*p as usize].area.is_some())
    }

    /// Scroll `pane` by `by` lines (up when negative), within what it was drawn with.
    pub fn scroll_pane(&mut self, pane: Pane, by: isize) {
        self.scrolls[pane as usize].scroll(by);
    }

    pub fn selected(&self) -> Option<&SessionRow> {
        self.results.get(self.list.selected)
    }

    /// The session enter, tab and ctrl-y act on: the fork highlighted in Inspect's expanded list
    /// of them, or else the selected session.
    pub fn target(&self) -> Option<&SessionRow> {
        if let Some(view) = self.expanded_children()
            && let Some(children) = self.children.get(&view.session)
            && let Some(&(_, i)) = panel::tree_order(&view.session, children).get(view.cursor)
        {
            return children.get(i);
        }
        self.selected()
    }

    /// Open on `row`, which the query named by id but which can't be resumed: it is selected, its
    /// plan is the reason (shown in the status row), and it stays first until the query changes.
    pub fn pin(&mut self, row: SessionRow, why: NotResumable) {
        self.status =
            Some((format!("can't resume {}: {why}", row.handle.session), Meaning::AlertError));
        self.plans.insert(row.handle.clone(), Err(why));
        self.pin_rows(vec![row]);
    }

    /// Open on `rows`, the sessions the query names by id (a prefix of several ids, or an id
    /// several agents have): they come first, the first selected, until the query changes.
    pub fn pin_matches(&mut self, rows: Vec<SessionRow>) {
        self.status = Some((
            format!("{} names {} sessions: pick one", self.input.as_str().trim(), rows.len()),
            Meaning::AlertInfo,
        ));
        self.pin_rows(rows);
    }

    fn pin_rows(&mut self, rows: Vec<SessionRow>) {
        self.results.clone_from(&rows);
        self.list.selected = 0;
        self.pinned = Some((rows, self.input.as_str().to_owned()));
    }

    /// Forget unanswered requests for sessions other than the selected one: the worker drops
    /// those once a newer request of the same kind arrives, so they must be asked for again. Not
    /// those of the session an enter, tab or ctrl-y waits on (a fork, in Inspect): only another
    /// action supersedes what it asked for, so asking again would only do it twice.
    pub fn forget_unanswered(&mut self) {
        let selected = self.selected().map(|r| r.handle.clone());
        let waiting = self.pending.as_ref().map(|(row, _)| row.handle.clone());
        self.requested.retain(|(handle, _)| {
            selected.as_ref() == Some(handle) || waiting.as_ref() == Some(handle)
        });
    }

    /// What the status row says: the latest message, else that the index is being rebuilt.
    pub fn status_line(&self) -> Option<(String, Meaning)> {
        self.status.clone().or_else(|| self.rebuilding.map(|r| (r.status(), Meaning::AlertWarn)))
    }

    // --- input ---------------------------------------------------------------------------------

    #[must_use]
    pub fn handle_input(&mut self, settings: &Settings, event: &Event) -> InputAction {
        match event {
            Event::Key(k) => self.handle_key_input(settings, k),
            Event::Mouse(m) => self.handle_mouse_input(settings, *m),
            Event::Paste(text) => {
                // Not into the query while the chooser is open: as with keys, it's the chooser's.
                if self.tab_index == 0 && self.chooser.is_none() {
                    for c in text.chars().filter(|c| !c.is_control()) {
                        self.input.insert(c);
                    }
                }
                InputAction::Continue
            }
            Event::Resize(..) => InputAction::Redraw,
            _ => InputAction::Continue,
        }
    }

    /// The wheel over a preview scrolls it; anywhere else (the list, the input) it moves the
    /// selection, as in the history search. The mouse reports screen positions, which is what
    /// the panes' areas are, inline or not.
    fn handle_mouse_input(&mut self, settings: &Settings, event: MouseEvent) -> InputAction {
        // The chooser is for the session it opened on.
        if self.chooser.is_some() {
            return InputAction::Continue;
        }
        let down = match event.kind {
            MouseEventKind::ScrollDown => true,
            MouseEventKind::ScrollUp => false,
            _ => return InputAction::Continue,
        };
        let at = Position::new(event.column, event.row);
        let over = PANES
            .into_iter()
            .find(|p| self.scrolls[*p as usize].area.is_some_and(|a| a.contains(at)));
        if let Some(pane) = over {
            let lines = isize::try_from(WHEEL_LINES).unwrap_or(1);
            self.scroll_pane(
                pane,
                if down {
                    lines
                } else {
                    -lines
                },
            );
            return InputAction::Continue;
        }
        let action = if down {
            Action::SelectNext
        } else {
            Action::SelectPrevious
        };
        self.execute_action(action, settings)
    }

    fn mode_keymap(&self) -> &Keymap {
        if self.tab_index == 1 {
            &self.keymaps.inspector
        } else {
            self.keymaps.for_mode(self.keymap_mode)
        }
    }

    fn is_insert_mode(&self) -> bool {
        self.tab_index == 0 && matches!(self.keymap_mode, KeymapMode::Emacs | KeymapMode::VimInsert)
    }

    #[must_use]
    pub fn handle_key_input(&mut self, settings: &Settings, input: &KeyEvent) -> InputAction {
        if input.kind == KeyEventKind::Release {
            return InputAction::Continue;
        }
        let Some(single) = SingleKey::from_event(input) else {
            return InputAction::Continue;
        };
        if self.chooser.is_some() {
            return self.chooser_key(&single);
        }
        let ctx = self.input.as_str().is_empty();
        let pending = self.pending_vim_key.take();
        let keymap = self.mode_keymap();

        let (action, new_pending) = if let Some(pending_char) = pending {
            let first = SingleKey {
                code: KeyCodeValue::Char(pending_char),
                ctrl: false,
                alt: false,
                shift: false,
                super_key: false,
            };
            let seq = KeyInput::Sequence(vec![first, single.clone()]);
            let action = keymap
                .resolve(&seq, ctx)
                .or_else(|| keymap.resolve(&KeyInput::Single(single.clone()), ctx));
            (action, None)
        } else if let KeyCodeValue::Char(c) = single.code
            && !single.ctrl
            && !single.alt
            && keymap.has_sequence_starting_with(&single)
        {
            (Some(Action::Noop), Some(c))
        } else {
            (keymap.resolve(&KeyInput::Single(single.clone()), ctx), None)
        };
        self.pending_vim_key = new_pending;

        if let Some(action) = action {
            return self.execute_action(action, settings);
        }
        if self.is_insert_mode() && !single.ctrl && !single.alt {
            match single.code {
                KeyCodeValue::Char(c) => self.input.insert(c),
                KeyCodeValue::Space => self.input.insert(' '),
                _ => {}
            }
        }
        InputAction::Continue
    }

    /// Move the selection toward index 0 (the best match, drawn at the bottom unless inverted).
    fn scroll_down(&mut self, n: usize) {
        self.list.selected = self.list.selected.saturating_sub(n);
    }

    fn scroll_up(&mut self, n: usize) {
        let last = self.results.len().saturating_sub(1);
        self.list.selected = (self.list.selected + n).min(last);
    }

    fn set_vim_mode(&mut self, mode: KeymapMode) {
        self.keymap_mode = mode;
    }

    /// The expanded children list, if it is the selected session's.
    pub fn expanded_children(&self) -> Option<&ChildrenView> {
        let selected = self.selected()?;
        self.children_view.as_ref().filter(|v| v.session == selected.handle && self.tab_index == 1)
    }

    /// `c` in Inspect: expand the selected session's children, or collapse them.
    fn toggle_children(&mut self) {
        if self.expanded_children().is_some() {
            self.children_view = None;
            return;
        }
        let Some(row) = self.selected() else {
            return;
        };
        // Until the forks are read, a row with anything grouped under it may have some.
        let forks = self
            .children
            .get(&row.handle)
            .map_or(usize::try_from(row.children).unwrap_or(1), Vec::len);
        if forks == 0 {
            return;
        }
        self.children_view = Some(ChildrenView {
            session: row.handle.clone(),
            cursor: 0,
            offset: 0,
            height: 1,
        });
    }

    /// A key while Inspect's children list is expanded: moving keys move in it, and esc
    /// collapses it. `None` for the keys it leaves alone.
    fn children_action(&mut self, action: Action) -> Option<InputAction> {
        let len = self
            .expanded_children()
            .map(|v| &v.session)
            .map(|s| self.children.get(s).map_or(0, Vec::len))?;
        let view = self.children_view.as_mut()?;
        let page = view.height.max(1);
        let last = len.saturating_sub(1);
        match action {
            Action::SelectNext => view.cursor = (view.cursor + 1).min(last),
            Action::SelectPrevious => view.cursor = view.cursor.saturating_sub(1),
            Action::ScrollPageDown => view.cursor = (view.cursor + page).min(last),
            Action::ScrollPageUp => view.cursor = view.cursor.saturating_sub(page),
            Action::ScrollHalfPageDown => view.cursor = (view.cursor + page / 2).min(last),
            Action::ScrollHalfPageUp => view.cursor = view.cursor.saturating_sub(page / 2),
            Action::ScrollToTop => view.cursor = 0,
            Action::ScrollToBottom => view.cursor = last,
            Action::Exit => self.children_view = None,
            _ => return None,
        }
        Some(InputAction::Continue)
    }

    #[allow(clippy::too_many_lines)]
    #[must_use]
    pub fn execute_action(&mut self, action: Action, settings: &Settings) -> InputAction {
        if let Some(outcome) = self.children_action(action) {
            return outcome;
        }
        let invert = settings.invert;
        let page = self.list.max_entries.saturating_sub(settings.scroll_context_lines).max(1);
        let words = (settings.word_chars.as_str(), settings.word_jump_mode);

        match action {
            Action::CursorLeft => {
                self.input.left();
            }
            Action::CursorRight => self.input.right(),
            Action::CursorWordLeft => self.input.prev_word(words.0, words.1),
            Action::CursorWordRight => self.input.next_word(words.0, words.1),
            Action::CursorWordEnd => self.input.word_end(words.0),
            Action::CursorStart => self.input.start(),
            Action::CursorEnd => self.input.end(),
            Action::DeleteCharBefore => {
                self.input.back();
            }
            Action::DeleteCharAfter => {
                self.input.remove();
            }
            Action::DeleteWordBefore => self.input.remove_prev_word(words.0, words.1),
            Action::DeleteWordAfter => self.input.remove_next_word(words.0, words.1),
            Action::DeleteToWordBoundary => {
                while matches!(self.input.back(), Some(c) if c.is_whitespace()) {}
                while self.input.left() {
                    if self.input.char().is_some_and(char::is_whitespace) {
                        self.input.right();
                        break;
                    }
                    self.input.remove();
                }
            }
            Action::ClearLine => self.input.clear(),
            Action::ClearToEnd => self.input.clear_to_end(),

            Action::SelectNext if invert => self.scroll_up(1),
            Action::SelectNext => self.scroll_down(1),
            Action::SelectPrevious if invert => self.scroll_down(1),
            Action::SelectPrevious => self.scroll_up(1),
            Action::ScrollPageDown if invert => self.scroll_up(page),
            Action::ScrollPageDown => self.scroll_down(page),
            Action::ScrollPageUp if invert => self.scroll_down(page),
            Action::ScrollPageUp => self.scroll_up(page),
            Action::ScrollHalfPageDown if invert => self.scroll_up(page / 2),
            Action::ScrollHalfPageDown => self.scroll_down(page / 2),
            Action::ScrollHalfPageUp if invert => self.scroll_down(page / 2),
            Action::ScrollHalfPageUp => self.scroll_up(page / 2),
            Action::ScrollToTop if invert => self.list.selected = 0,
            Action::ScrollToTop => self.list.selected = self.results.len().saturating_sub(1),
            Action::ScrollToBottom if invert => {
                self.list.selected = self.results.len().saturating_sub(1);
            }
            Action::ScrollToBottom => self.list.selected = 0,

            Action::PreviewUp
            | Action::PreviewDown
            | Action::PreviewPageUp
            | Action::PreviewPageDown => {
                if let Some(pane) = self.drawn_pane() {
                    let page = self.scrolls[pane as usize].height.saturating_sub(1).max(1);
                    let page = isize::try_from(page).unwrap_or(1);
                    let by = match action {
                        Action::PreviewUp => -1,
                        Action::PreviewDown => 1,
                        Action::PreviewPageUp => -page,
                        _ => page,
                    };
                    self.scroll_pane(pane, by);
                }
            }

            Action::Resume => return InputAction::Resume,
            Action::ReturnCommand => return InputAction::ReturnCommand,
            Action::Copy => return InputAction::Copy,
            Action::Fork => return InputAction::Fork,
            Action::ReturnOriginal => return InputAction::ReturnOriginal,
            // Nothing to clear: every frame is drawn whole, and clearing the terminal first only
            // flashed it blank.
            Action::Exit if self.tab_index == 1 => self.tab_index = 0,
            Action::Exit => return InputAction::Exit,
            Action::Redraw => return InputAction::Redraw,
            Action::CycleFilterMode => self.cycle_filter_mode(),
            Action::CycleAgent => {
                self.input = Cursor::from(query::cycle_harness(self.input.as_str()));
                self.input.end();
            }
            Action::ToggleTab => {
                // Search and Inspect.
                self.tab_index = 1 - self.tab_index;
                self.children_view = None;
            }
            Action::ToggleChildren => self.toggle_children(),

            Action::VimEnterNormal => self.set_vim_mode(KeymapMode::VimNormal),
            Action::VimEnterInsert => self.set_vim_mode(KeymapMode::VimInsert),
            Action::VimEnterInsertAfter => {
                self.input.right();
                self.set_vim_mode(KeymapMode::VimInsert);
            }
            Action::VimEnterInsertAtStart => {
                self.input.start();
                self.set_vim_mode(KeymapMode::VimInsert);
            }
            Action::VimEnterInsertAtEnd => {
                self.input.end();
                self.set_vim_mode(KeymapMode::VimInsert);
            }
            Action::VimSearchInsert => {
                self.input.clear();
                self.set_vim_mode(KeymapMode::VimInsert);
            }
            Action::VimChangeToEnd => {
                self.input.clear_to_end();
                self.set_vim_mode(KeymapMode::VimInsert);
            }
            Action::Noop => {}
        }
        InputAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use atuin_client::ai_session::HarnessKind;
    use crossterm::event::{KeyCode, KeyModifiers};
    use rstest::rstest;

    use super::*;
    use crate::resume_tui::fake;

    fn settings() -> Settings {
        Settings::utc()
    }

    fn state_in(context: ResumeContext) -> State {
        State::new(&settings(), context, "")
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn rows(n: usize) -> Vec<SessionRow> {
        (0..n).map(|i| fake::row(HarnessKind::ClaudeCode, &format!("s{i}"), "t")).collect()
    }

    /// A picker opened with `filter_mode` as `configured`, in a repository on a branch, on a
    /// detached `HEAD`, or outside one.
    fn opened(configured: Option<FilterMode>, repo: bool, branch: bool) -> State {
        let mut settings = settings();
        settings.ai.sessions.filter_mode = configured;
        let mut ctx = fake::context();
        if !repo {
            ctx.git_root = None;
        }
        if !branch {
            ctx.branch = None;
        }
        State::new(&settings, ctx, "")
    }

    /// The configured scope, when it can apply here; else, for a branch on a detached `HEAD`, the
    /// repository's; else every session's.
    #[rstest]
    #[case::unset(None, true, true, FilterMode::Global)]
    #[case::workspace(Some(FilterMode::Workspace), true, true, FilterMode::Workspace)]
    #[case::workspace_outside_a_repo(Some(FilterMode::Workspace), false, false, FilterMode::Global)]
    #[case::branch_on_a_detached_head(Some(FilterMode::Branch), true, false, FilterMode::Workspace)]
    #[case::branch_outside_a_repo(Some(FilterMode::Branch), false, false, FilterMode::Global)]
    fn the_picker_opens_on(
        #[case] configured: Option<FilterMode>,
        #[case] repo: bool,
        #[case] branch: bool,
        #[case] want: FilterMode,
    ) {
        assert_eq!(opened(configured, repo, branch).mode, want);
    }

    #[rstest]
    fn an_empty_workspace_opens_on_every_session_once() {
        let mut state = opened(Some(FilterMode::Workspace), true, true);
        assert_eq!(state.filter().db.workspace, Some(PathBuf::from(fake::REPO)));
        let (generation, _) = state.next_search().unwrap();
        state.apply_results(generation, Vec::new());
        assert_eq!(state.mode, FilterMode::Global);
        assert!(state.status_line().is_some_and(|(s, _)| s == FELL_BACK));

        let (generation, filter) = state.next_search().unwrap();
        assert_eq!(filter.db.workspace, None);
        state.apply_results(generation, rows(2));
        assert_eq!(state.results.len(), 2);

        // ctrl-r moves the scope: what the fallback said no longer holds.
        state.cycle_filter_mode();
        assert_eq!(state.status_line(), None);
    }

    /// Only an opening with nothing to narrow it (no text, no chip) says whether the workspace
    /// has sessions; a later query matching nothing stays put.
    #[rstest]
    #[case::text("flaky")]
    #[case::a_chip("b:no-such-branch")]
    #[case::an_agent("a:pi")]
    fn a_narrowed_workspace_stays_workspace(#[case] query: &str) {
        let mut settings = settings();
        settings.ai.sessions.filter_mode = Some(FilterMode::Workspace);
        let mut state = State::new(&settings, fake::context(), query);
        let (generation, _) = state.next_search().unwrap();
        state.apply_results(generation, Vec::new());
        assert_eq!(state.mode, FilterMode::Workspace);
        assert_eq!(state.status_line(), None);
    }

    #[rstest]
    fn workspace_picked_with_ctrl_r_never_falls_back() {
        let mut state = opened(Some(FilterMode::Workspace), true, true);
        state.cycle_filter_mode();
        state.mode = FilterMode::Workspace;
        let (generation, _) = state.next_search().unwrap();
        state.apply_results(generation, Vec::new());
        assert_eq!(state.mode, FilterMode::Workspace);
    }

    #[rstest]
    fn ctrl_r_cycles_available_modes() {
        let mut state = state_in(fake::context());
        let s = settings();
        let seen: Vec<_> = (0..5)
            .map(|_| {
                let _ = state.handle_input(&s, &key(KeyCode::Char('r'), KeyModifiers::CONTROL));
                state.mode
            })
            .collect();
        // From every session narrowing to the workspace, its branch and the directory.
        assert_eq!(seen, vec![
            FilterMode::Workspace,
            FilterMode::Branch,
            FilterMode::Directory,
            FilterMode::Host,
            FilterMode::Global,
        ]);

        let mut state = opened(Some(FilterMode::Workspace), false, false);
        state.cycle_filter_mode();
        assert_eq!(state.mode, FilterMode::Directory);
        state.cycle_filter_mode();
        assert_eq!(state.mode, FilterMode::Host);
        state.cycle_filter_mode();
        // Workspace and branch are skipped without a repository.
        assert_eq!(state.mode, FilterMode::Global);
    }

    #[rstest]
    fn modes_resolve_to_filters() {
        let mut state = state_in(fake::context());
        state.mode = FilterMode::Host;
        let host = state.filter().db.host.map(|h| h.0.as_simple().to_string());
        assert_eq!(host.as_deref(), Some(fake::THIS_HOST_ID));
        assert!(state.filter().db.or_unrecorded);
        state.mode = FilterMode::Directory;
        assert_eq!(state.filter().db.directory, Some(PathBuf::from(fake::REPO)));
        state.mode = FilterMode::Branch;
        assert_eq!(state.filter().db.branch.as_deref(), Some("ai-resume"));
        state.input = Cursor::from("b:main flaky agent:codex".to_owned());
        let filter = state.filter();
        assert_eq!(filter.db.branch.as_deref(), Some("main"));
        assert_eq!(filter.text, "flaky");
        assert_eq!(filter.db.harness, Some(HarnessKind::Codex));
        assert!(filter.db.roots_only);
    }

    #[rstest]
    fn stale_generations_are_dropped_and_the_old_list_kept() {
        let mut state = state_in(fake::context());
        let (g1, _) = state.next_search().unwrap();
        state.apply_results(g1, rows(3));
        state.input = Cursor::from("a".to_owned());
        let (g2, _) = state.next_search().unwrap();
        state.input = Cursor::from("ab".to_owned());
        let (g3, _) = state.next_search().unwrap();
        assert!(g1 < g2 && g2 < g3);

        assert!(!state.apply_results(g2, rows(1)));
        assert_eq!(state.results.len(), 3, "old list stays until the newest answers");
        assert_eq!(state.applied, g1);
        assert!(state.apply_results(g3, rows(2)));
        assert_eq!(state.results.len(), 2);
        assert_eq!(state.applied, g3);
    }

    #[rstest]
    fn refresh_keeps_the_selection_and_reloads_live_previews() {
        let mut state = state_in(fake::context());
        state.now = Box::new(fake::now);
        let (g, _) = state.next_search().unwrap();
        let mut rs = rows(3);
        rs[2].updated_at = fake::now();
        state.apply_results(g, rs.clone());
        state.list.selected = 1;
        for r in &rs {
            state.previews.insert(r.handle.clone(), SessionPreview::default());
        }

        let (g, _) = state.refresh().unwrap();
        assert!(state.refresh().is_none(), "one refresh at a time");
        // The live preview is read again, and shown as it was meanwhile.
        assert!(state.wants_preview(&rs[2].handle), "the live preview reloads");
        assert!(state.previews.contains_key(&rs[2].handle), "and is kept until it has");
        assert!(!state.wants_preview(&rs[0].handle));
        let fresh = SessionPreview {
            first_prompt: Some("new".to_owned()),
            ..SessionPreview::default()
        };
        state.apply_preview(rs[2].handle.clone(), fresh.clone());
        assert!(!state.wants_preview(&rs[2].handle));
        assert_eq!(state.previews[&rs[2].handle], fresh);
        // The list reorders; the selection follows its session.
        rs.swap(0, 1);
        state.apply_results(g, rs.clone());
        assert_eq!(state.list.selected, 0);
        assert_eq!(state.selected().unwrap().handle, rs[0].handle);

        // A new query starts at the top again.
        state.list.selected = 2;
        state.input = Cursor::from("x".to_owned());
        let (g, _) = state.next_search().unwrap();
        state.apply_results(g, rs);
        assert_eq!(state.list.selected, 0);
    }

    #[rstest]
    fn a_pinned_session_stays_first_until_the_query_changes() {
        let mut state = State::new(&settings(), fake::context(), "s7a1b2c");
        let pinned = fake::row(HarnessKind::ClaudeCode, "s7a1b2c", "t");
        state.pin(pinned.clone(), NotResumable::NotInstalled("claude".to_owned()));
        assert_eq!(state.selected(), Some(&pinned));
        assert!(state.status.as_ref().is_some_and(|(s, _)| s.contains("isn't installed here")));

        // The id matches no text; the pinned row stays.
        let (generation, _) = state.next_search().unwrap();
        state.apply_results(generation, Vec::new());
        assert_eq!(state.results, vec![pinned.clone()]);
        // A refresh that finds it too still shows it once, first.
        state.apply_results(generation, vec![rows(2)[1].clone(), pinned.clone()]);
        assert_eq!(state.results[0], pinned);
        assert_eq!(state.results.len(), 2);

        state.input = Cursor::from("other".to_owned());
        let (generation, _) = state.next_search().unwrap();
        state.apply_results(generation, rows(2));
        assert!(!state.results.contains(&pinned));
        assert_eq!(state.status, None);
    }

    /// An id several sessions have opens on all of them, the first selected, ahead of anything
    /// the search finds, until the query changes.
    #[rstest]
    fn the_sessions_an_id_names_stay_first_until_the_query_changes() {
        let mut state = State::new(&settings(), fake::context(), "7aaabc31");
        let claude = fake::row(HarnessKind::ClaudeCode, "7aaabc31-1631", "a");
        let pi = fake::row(HarnessKind::Pi, "7aaabc31-1631", "b");
        state.pin_matches(vec![claude.clone(), pi.clone()]);
        assert_eq!(state.results, vec![claude.clone(), pi.clone()]);
        assert_eq!(state.selected(), Some(&claude));
        assert!(
            state.status.as_ref().is_some_and(|(s, _)| s == "7aaabc31 names 2 sessions: pick one")
        );
        // Nothing is planned for them up front: either may be resumable.
        assert!(state.plans.is_empty());

        let (generation, _) = state.next_search().unwrap();
        let other = rows(2).remove(1);
        state.apply_results(generation, vec![pi.clone(), other.clone()]);
        assert_eq!(state.results, vec![claude.clone(), pi, other]);

        state.input = Cursor::from("other".to_owned());
        let (generation, _) = state.next_search().unwrap();
        state.apply_results(generation, rows(2));
        assert!(!state.results.contains(&claude));
        assert_eq!(state.status, None);
    }

    /// Moving to a session not read yet holds the last preview shown, from the first frame drawn
    /// without it, until the new one is read or [`HOLD`] passes; never an empty preview between.
    #[rstest]
    fn the_last_preview_is_held_until_the_next_is_read() {
        let mut state = state_in(fake::context());
        state.results = rows(3);
        let [a, b, c] = [0, 1, 2].map(|i| state.results[i].handle.clone());
        let t0 = Instant::now();
        state.apply_preview(a.clone(), SessionPreview::default());
        state.preview_drawn(t0);

        state.list.selected = 1;
        // Long after the last frame drawn with it selected: the hold starts with the move.
        let t1 = t0 + 10 * HOLD;
        assert_eq!(state.preview_row_at(t1).unwrap().handle, a);
        state.preview_drawn(t1);
        state.list.selected = 2;
        state.preview_drawn(t1 + HOLD / 2);
        assert_eq!(state.preview_row_at(t1 + HOLD / 2).unwrap().handle, a);
        assert_eq!(state.preview_row_at(t1 + HOLD).unwrap().handle, c, "then the selected");

        state.apply_preview(b.clone(), SessionPreview::default());
        state.list.selected = 1;
        assert_eq!(state.preview_row_at(t1 + HOLD).unwrap().handle, b);
        state.preview_drawn(t1 + HOLD);
        state.list.selected = 2;
        assert_eq!(state.preview_row_at(t1 + 2 * HOLD).unwrap().handle, b);
    }

    /// The wheel scrolls within the text drawn: not above its top, nor past its end once all of
    /// it is rendered; while more is, past what was rendered, for the next frame to render it.
    #[rstest]
    fn panes_scroll_within_their_text() {
        let mut scroll = PaneScroll {
            session: Some(rows(1)[0].handle.clone()),
            height: 4,
            len: 10,
            ..PaneScroll::default()
        };
        scroll.scroll(-3);
        assert_eq!(scroll.offset, 0);
        scroll.scroll(100);
        assert_eq!(scroll.offset, 6);
        scroll.more = true;
        scroll.scroll(100);
        assert_eq!(scroll.offset, 10);
        assert_eq!(scroll.offset_for(&rows(2)[1].handle), 0, "another session's is the top");
    }

    #[rstest]
    fn forgetting_unanswered_requests_keeps_the_selected_sessions() {
        let mut state = state_in(fake::context());
        state.results = rows(3);
        for row in rows(3) {
            state.requested.insert((row.handle, PREVIEW));
        }
        state.list.selected = 1;
        state.forget_unanswered();
        let kept: Vec<_> = state.requested.iter().map(|(h, _)| h.clone()).collect();
        assert_eq!(kept, vec![rows(3)[1].handle.clone()]);

        // And the session an action waits on.
        let fork = rows(3)[2].handle.clone();
        state.requested.insert((fork.clone(), PLAN));
        state.pending = Some((rows(3)[2].clone(), Pending::Resume));
        state.forget_unanswered();
        assert!(state.requested.contains(&(fork, PLAN)));
        assert_eq!(state.requested.len(), 2);
    }

    #[rstest]
    fn unchanged_filter_does_not_search_again() {
        let mut state = state_in(fake::context());
        assert!(state.next_search().is_some());
        assert!(state.next_search().is_none());
        state.input.insert(' ');
        // Whitespace doesn't change the parsed filter.
        assert!(state.next_search().is_none());
    }

    #[rstest]
    fn alt_a_rewrites_the_input() {
        let mut state = state_in(fake::context());
        let s = settings();
        state.input = Cursor::from("flaky".to_owned());
        let _ = state.handle_input(&s, &key(KeyCode::Char('a'), KeyModifiers::ALT));
        assert_eq!(state.input.as_str(), "flaky agent:claude-code");
        let _ = state.handle_input(&s, &key(KeyCode::Char('a'), KeyModifiers::ALT));
        assert_eq!(state.input.as_str(), "flaky agent:codex");
        // The short form is cycled too, and comes back as the long one.
        state.input = Cursor::from("a:opencode flaky".to_owned());
        let _ = state.handle_input(&s, &key(KeyCode::Char('a'), KeyModifiers::ALT));
        assert_eq!(state.input.as_str(), "agent:pi flaky");
        // alt-h is unbound: it neither cycles nor types.
        let _ = state.handle_input(&s, &key(KeyCode::Char('h'), KeyModifiers::ALT));
        assert_eq!(state.input.as_str(), "agent:pi flaky");
    }

    #[rstest]
    fn keys_map_to_outcomes() {
        let mut s = settings();
        s.enter_accept = true;
        let mut state = State::new(&s, fake::context(), "");
        state.results = rows(3);
        state.list.selected = 1;

        let press = |state: &mut State, code, m| state.handle_input(&s, &key(code, m));
        assert_eq!(press(&mut state, KeyCode::Enter, KeyModifiers::NONE), InputAction::Resume);
        assert_eq!(press(&mut state, KeyCode::Tab, KeyModifiers::NONE), InputAction::ReturnCommand);
        assert_eq!(press(&mut state, KeyCode::Char('y'), KeyModifiers::CONTROL), InputAction::Copy);
        assert_eq!(state.target(), Some(&state.results[1]));
        assert_eq!(
            press(&mut state, KeyCode::Char('c'), KeyModifiers::CONTROL),
            InputAction::ReturnOriginal
        );
        assert_eq!(
            press(&mut state, KeyCode::Char('g'), KeyModifiers::CONTROL),
            InputAction::ReturnOriginal
        );
        assert_eq!(press(&mut state, KeyCode::Esc, KeyModifiers::NONE), InputAction::Exit);

        // ctrl-o opens Inspect; esc there goes back instead of exiting. Neither clears the
        // terminal (a blank frame, then the whole screen drawn again).
        assert_eq!(
            press(&mut state, KeyCode::Char('o'), KeyModifiers::CONTROL),
            InputAction::Continue
        );
        assert_eq!(state.tab_index, 1);
        assert_eq!(press(&mut state, KeyCode::Esc, KeyModifiers::NONE), InputAction::Continue);
        assert_eq!(state.tab_index, 0);
        assert_eq!(
            press(&mut state, KeyCode::Char('l'), KeyModifiers::CONTROL),
            InputAction::Redraw
        );
    }

    #[rstest]
    fn typing_inserts_and_navigation_respects_invert() {
        let mut s = settings();
        let mut state = State::new(&s, fake::context(), "");
        state.results = rows(5);
        for c in "bug".chars() {
            let _ = state.handle_input(&s, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(state.input.as_str(), "bug");

        // Not inverted: the best match is at the bottom, so up moves away from it.
        let _ = state.handle_input(&s, &key(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(state.list.selected, 1);
        let _ = state.handle_input(&s, &key(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(state.list.selected, 0);

        s.invert = true;
        let _ = state.handle_input(&s, &key(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(state.list.selected, 1);
    }

    #[rstest]
    fn vim_modes() {
        let mut s = settings();
        s.keymap_mode = KeymapMode::VimInsert;
        let mut state = State::new(&s, fake::context(), "");
        state.results = rows(5);
        let _ = state.handle_input(&s, &key(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(state.input.as_str(), "k");
        let _ = state.handle_input(&s, &key(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(state.keymap_mode, KeymapMode::VimNormal);
        let _ = state.handle_input(&s, &key(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(state.list.selected, 1);
        let _ = state.handle_input(&s, &key(KeyCode::Char('g'), KeyModifiers::NONE));
        let _ = state.handle_input(&s, &key(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(state.list.selected, 4, "gg jumps to the visual top");
        let _ = state.handle_input(&s, &key(KeyCode::Char('i'), KeyModifiers::NONE));
        assert_eq!(state.keymap_mode, KeymapMode::VimInsert);
    }
}
