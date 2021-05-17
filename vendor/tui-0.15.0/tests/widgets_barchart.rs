use tui::backend::TestBackend;
use tui::buffer::Buffer;
use tui::widgets::{BarChart, Block, Borders};
use tui::Terminal;

#[test]
fn widgets_barchart_not_full_below_max_value() {
    let test_case = |expected| {
        let backend = TestBackend::new(30, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                let size = f.size();
                let barchart = BarChart::default()
                    .block(Block::default().borders(Borders::ALL))
                    .data(&[("empty", 0), ("half", 50), ("almost", 99), ("full", 100)])
                    .max(100)
                    .bar_width(7)
                    .bar_gap(0);
                f.render_widget(barchart, size);
            })
            .unwrap();
        terminal.backend().assert_buffer(&expected);
    };

    // check that bars fill up correctly up to max value
    test_case(Buffer::with_lines(vec![
        "┌────────────────────────────┐",
        "│              ▇▇▇▇▇▇▇███████│",
        "│              ██████████████│",
        "│              ██████████████│",
        "│       ▄▄▄▄▄▄▄██████████████│",
        "│       █████████████████████│",
        "│       █████████████████████│",
        "│       ██50█████99█████100██│",
        "│empty  half   almost full   │",
        "└────────────────────────────┘",
    ]));
}
