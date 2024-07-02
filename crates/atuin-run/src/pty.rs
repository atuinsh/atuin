/// Create and manage pseudoterminals
use std::{
    io::{self, BufWriter, Read, Write},
    sync::{Arc, RwLock},
    time::Duration,
};

use bytes::Bytes;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    style::ResetColor,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::{
    sync::mpsc::{channel, Sender},
    task,
};
use tui_term::widget::PseudoTerminal;
use vt100::Screen;

#[derive(Debug)]
struct Size {
    cols: u16,
    rows: u16,
}

pub async fn run_pty(code: String) -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, ResetColor)?;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let pty_system = NativePtySystem::default();
    let cwd = std::env::current_dir().unwrap();
    let mut cmd = CommandBuilder::new_default_prog();
    cmd.cwd(cwd);

    let size = Size {
        rows: terminal.size()?.height,
        cols: terminal.size()?.width,
    };

    let pair = pty_system
        .openpty(PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    // Wait for the child to complete
    task::spawn_blocking(move || {
        let mut child = pair.slave.spawn_command(cmd).unwrap();
        let _child_exit_status = child.wait().unwrap();
        drop(pair.slave);
    });

    let mut reader = pair.master.try_clone_reader().unwrap();
    let parser = Arc::new(RwLock::new(vt100::Parser::new(size.rows, size.cols, 0)));

    {
        let parser = parser.clone();
        task::spawn_blocking(move || {
            // Consume the output from the child
            // Can't read the full buffer, since that would wait for EOF
            let mut buf = [0u8; 8192];
            let mut processed_buf = Vec::new();
            loop {
                let size = reader.read(&mut buf).unwrap();
                if size == 0 {
                    break;
                }
                if size > 0 {
                    processed_buf.extend_from_slice(&buf[..size]);
                    let mut parser = parser.write().unwrap();
                    parser.process(&processed_buf);

                    // Clear the processed portion of the buffer
                    processed_buf.clear();
                }
            }
        });
    }

    let (tx, mut rx) = channel::<Bytes>(32);

    let mut writer = BufWriter::new(pair.master.take_writer().unwrap());

    // Drop writer on purpose
    tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            writer.write_all(&bytes).unwrap();
            writer.flush().unwrap();
        }
        drop(pair.master);
    });

    for line in code.lines() {
        let b = Bytes::from(line.trim_end().to_string().into_bytes());

        sender.send(b).await;
        sender.send(Bytes::from(vec![b'\n'])).await;
    }

    tokio::time::sleep(Duration::from_secs(2)).await;

    // restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen,)?;
    terminal.show_cursor()?;

    Ok(())
}

async fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    parser: Arc<RwLock<vt100::Parser>>,
    sender: Sender<Bytes>,
    blocks: Vec<crate::markdown::Block>,
) -> io::Result<()> {
    tokio::time::sleep(Duration::from_millis(500)).await;

    for i in blocks {
        terminal.draw(|f| ui(f, parser.read().unwrap().screen()))?;

        tokio::time::sleep(Duration::from_millis(500)).await;

    }

    terminal.draw(|f| ui(f, parser.read().unwrap().screen()))?;

    tokio::time::sleep(Duration::from_millis(1500)).await;

    Ok(())
}

fn ui(f: &mut Frame, screen: &Screen) {
    let chunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .margin(1)
        .constraints(
            [
                ratatui::layout::Constraint::Percentage(100),
                ratatui::layout::Constraint::Min(1),
            ]
            .as_ref(),
        )
        .split(f.size());
    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().add_modifier(Modifier::BOLD));

    let pseudo_term = PseudoTerminal::new(screen).block(block);
    f.render_widget(pseudo_term, chunks[0]);

    let explanation = "Press q to exit".to_string();
    let explanation = Paragraph::new(explanation)
        .style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED))
        .alignment(Alignment::Center);
    f.render_widget(explanation, chunks[1]);
}
