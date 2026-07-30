use std::future::Future;
use std::io;
use std::pin::Pin;
use std::time::Duration;

use ratatui::{
    Frame, TerminalOptions, Viewport,
    crossterm::event::{self, Event, KeyEventKind},
};
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::msg::Msg;

/// A side effect requested by [`App::update`]. The runtime interprets it: spawn
/// async work whose result re-enters `update` as a `Msg`, quit, or nothing.
///
/// This is the effect channel of the architecture — `update` stays synchronous
/// and pure, describing async work as data rather than performing it.
pub enum Cmd {
    /// Do nothing.
    None,
    /// Exit the runtime loop.
    Quit,
    /// Run several commands.
    Batch(Vec<Cmd>),
    /// Spawn an async task; its output message is folded back into `update`.
    Task(Pin<Box<dyn Future<Output = Msg> + Send + 'static>>),
}

impl Cmd {
    /// A command that runs `fut` and folds its result back in as a message.
    pub fn task(fut: impl Future<Output = Msg> + Send + 'static) -> Self {
        Cmd::Task(Box::pin(fut))
    }
}

/// A TEA application: state folded by [`update`](App::update) and rendered by
/// [`view`](App::view). The runtime owns the terminal and the loop; the app owns
/// the model. Both methods stay synchronous — async work is expressed as [`Cmd`].
pub trait App {
    /// Command to run once at startup, before the first message (e.g. kick off
    /// initial data loads). Defaults to nothing.
    fn init(&mut self) -> Cmd {
        Cmd::None
    }

    /// Fold a message into state and return a command to run. The only place
    /// application state changes.
    fn update(&mut self, msg: Msg) -> Cmd;

    /// Render the current state. Takes `&mut self` only to lend render caches
    /// (e.g. the image protocol) to stateful widgets — state changes solely in
    /// [`update`](App::update).
    fn view(&mut self, frame: &mut Frame<'_>);
}

/// Run `app` fullscreen, in the alternate screen buffer.
///
/// Requires a tokio runtime. Any terminal querying an app needs (e.g.
/// image-protocol detection) must happen *before* calling this.
pub async fn run<A: App>(app: A) -> io::Result<()> {
    drive(app, ratatui::init()).await
}

/// Run `app` inline, occupying `height` rows in the normal terminal buffer
/// (scrollback) rather than the alternate screen.
pub async fn run_inline<A: App>(app: A, height: u16) -> io::Result<()> {
    let terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(height),
    });
    drive(app, terminal).await
}

async fn drive<A: App>(app: A, terminal: ratatui::DefaultTerminal) -> io::Result<()> {
    let result = event_loop(app, terminal).await;
    ratatui::restore();
    result
}

async fn event_loop<A: App>(mut app: A, mut terminal: ratatui::DefaultTerminal) -> io::Result<()> {
    // Every input event and every task result arrives as a `Msg` on this one
    // channel; the loop is just render → recv → update.
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    spawn_input_reader(tx.clone());
    spawn_cmd(app.init(), &tx);

    loop {
        terminal.draw(|frame| app.view(frame))?;

        let Some(msg) = rx.recv().await else {
            break; // all senders gone
        };
        match app.update(msg) {
            Cmd::Quit => break,
            cmd => spawn_cmd(cmd, &tx),
        }
    }
    Ok(())
}

/// Interpret a command: spawn its async work so the result message is sent back
/// into the loop. Quit is handled by the caller.
fn spawn_cmd(cmd: Cmd, tx: &UnboundedSender<Msg>) {
    match cmd {
        Cmd::None | Cmd::Quit => {}
        Cmd::Batch(cmds) => {
            for cmd in cmds {
                spawn_cmd(cmd, tx);
            }
        }
        Cmd::Task(fut) => {
            let tx = tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(fut.await);
            });
        }
    }
}

/// Read terminal events on a dedicated thread and forward them as messages,
/// exiting once the runtime drops the receiver.
fn spawn_input_reader(tx: UnboundedSender<Msg>) {
    std::thread::spawn(move || {
        loop {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(event) => {
                        if let Some(msg) = msg_from_event(event)
                            && tx.send(msg).is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                // Nothing to read — bail out if the loop is gone, else keep polling.
                Ok(false) if tx.is_closed() => break,
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });
}

fn msg_from_event(event: Event) -> Option<Msg> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => Some(Msg::Key(key)),
        Event::Resize(cols, rows) => Some(Msg::Resize(cols, rows)),
        _ => None,
    }
}
