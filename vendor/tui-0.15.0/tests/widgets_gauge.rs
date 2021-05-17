use tui::{
    backend::TestBackend,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    symbols,
    widgets::{Block, Borders, Gauge, LineGauge},
    Terminal,
};

#[test]
fn widgets_gauge_renders() {
    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(2)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                .split(f.size());

            let gauge = Gauge::default()
                .block(Block::default().title("Percentage").borders(Borders::ALL))
                .gauge_style(Style::default().bg(Color::Blue).fg(Color::Red))
                .use_unicode(true)
                .percent(43);
            f.render_widget(gauge, chunks[0]);
            let gauge = Gauge::default()
                .block(Block::default().title("Ratio").borders(Borders::ALL))
                .gauge_style(Style::default().bg(Color::Blue).fg(Color::Red))
                .use_unicode(true)
                .ratio(0.511_313_934_313_1);
            f.render_widget(gauge, chunks[1]);
        })
        .unwrap();
    let mut expected = Buffer::with_lines(vec![
        "                                        ",
        "                                        ",
        "  ┌Percentage────────────────────────┐  ",
        "  │              ▋43%                │  ",
        "  └──────────────────────────────────┘  ",
        "  ┌Ratio─────────────────────────────┐  ",
        "  │               51%                │  ",
        "  └──────────────────────────────────┘  ",
        "                                        ",
        "                                        ",
    ]);

    for i in 3..17 {
        expected
            .get_mut(i, 3)
            .set_bg(Color::Red)
            .set_fg(Color::Blue);
    }
    for i in 17..37 {
        expected
            .get_mut(i, 3)
            .set_bg(Color::Blue)
            .set_fg(Color::Red);
    }

    for i in 3..20 {
        expected
            .get_mut(i, 6)
            .set_bg(Color::Red)
            .set_fg(Color::Blue);
    }
    for i in 20..37 {
        expected
            .get_mut(i, 6)
            .set_bg(Color::Blue)
            .set_fg(Color::Red);
    }

    terminal.backend().assert_buffer(&expected);
}

#[test]
fn widgets_gauge_renders_no_unicode() {
    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal
        .draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(2)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                .split(f.size());

            let gauge = Gauge::default()
                .block(Block::default().title("Percentage").borders(Borders::ALL))
                .percent(43)
                .use_unicode(false);
            f.render_widget(gauge, chunks[0]);
            let gauge = Gauge::default()
                .block(Block::default().title("Ratio").borders(Borders::ALL))
                .ratio(0.211_313_934_313_1)
                .use_unicode(false);
            f.render_widget(gauge, chunks[1]);
        })
        .unwrap();
    let expected = Buffer::with_lines(vec![
        "                                        ",
        "                                        ",
        "  ┌Percentage────────────────────────┐  ",
        "  │               43%                │  ",
        "  └──────────────────────────────────┘  ",
        "  ┌Ratio─────────────────────────────┐  ",
        "  │               21%                │  ",
        "  └──────────────────────────────────┘  ",
        "                                        ",
        "                                        ",
    ]);
    terminal.backend().assert_buffer(&expected);
}

#[test]
fn widgets_line_gauge_renders() {
    let backend = TestBackend::new(20, 4);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let gauge = LineGauge::default()
                .gauge_style(Style::default().fg(Color::Green).bg(Color::White))
                .ratio(0.43);
            f.render_widget(
                gauge,
                Rect {
                    x: 0,
                    y: 0,
                    width: 20,
                    height: 1,
                },
            );
            let gauge = LineGauge::default()
                .block(Block::default().title("Gauge 2").borders(Borders::ALL))
                .gauge_style(Style::default().fg(Color::Green))
                .line_set(symbols::line::THICK)
                .ratio(0.211_313_934_313_1);
            f.render_widget(
                gauge,
                Rect {
                    x: 0,
                    y: 1,
                    width: 20,
                    height: 3,
                },
            );
        })
        .unwrap();
    let mut expected = Buffer::with_lines(vec![
        "43% ────────────────",
        "┌Gauge 2───────────┐",
        "│21% ━━━━━━━━━━━━━━│",
        "└──────────────────┘",
    ]);
    for col in 4..10 {
        expected.get_mut(col, 0).set_fg(Color::Green);
    }
    for col in 10..20 {
        expected.get_mut(col, 0).set_fg(Color::White);
    }
    for col in 5..7 {
        expected.get_mut(col, 2).set_fg(Color::Green);
    }
    terminal.backend().assert_buffer(&expected);
}
