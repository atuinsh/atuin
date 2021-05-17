use tui::{
    backend::TestBackend, buffer::Buffer, layout::Rect, symbols, text::Spans, widgets::Tabs,
    Terminal,
};

#[test]
fn widgets_tabs_should_not_panic_on_narrow_areas() {
    let backend = TestBackend::new(1, 1);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let tabs = Tabs::new(["Tab1", "Tab2"].iter().cloned().map(Spans::from).collect());
            f.render_widget(
                tabs,
                Rect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
            );
        })
        .unwrap();
    let expected = Buffer::with_lines(vec![" "]);
    terminal.backend().assert_buffer(&expected);
}

#[test]
fn widgets_tabs_should_truncate_the_last_item() {
    let backend = TestBackend::new(10, 1);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let tabs = Tabs::new(["Tab1", "Tab2"].iter().cloned().map(Spans::from).collect());
            f.render_widget(
                tabs,
                Rect {
                    x: 0,
                    y: 0,
                    width: 9,
                    height: 1,
                },
            );
        })
        .unwrap();
    let expected = Buffer::with_lines(vec![format!(" Tab1 {} T ", symbols::line::VERTICAL)]);
    terminal.backend().assert_buffer(&expected);
}
