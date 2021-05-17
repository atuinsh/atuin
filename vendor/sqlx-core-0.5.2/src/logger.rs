use crate::connection::LogSettings;
use std::time::Instant;

pub(crate) struct QueryLogger<'q> {
    sql: &'q str,
    rows: usize,
    start: Instant,
    settings: LogSettings,
}

impl<'q> QueryLogger<'q> {
    pub(crate) fn new(sql: &'q str, settings: LogSettings) -> Self {
        Self {
            sql,
            rows: 0,
            start: Instant::now(),
            settings,
        }
    }

    pub(crate) fn increment_rows(&mut self) {
        self.rows += 1;
    }

    pub(crate) fn finish(&self) {
        let elapsed = self.start.elapsed();

        let lvl = if elapsed >= self.settings.slow_statements_duration {
            self.settings.slow_statements_level
        } else {
            self.settings.statements_level
        };

        if let Some(lvl) = lvl
            .to_level()
            .filter(|lvl| log::log_enabled!(target: "sqlx::query", *lvl))
        {
            let mut summary = parse_query_summary(&self.sql);

            let sql = if summary != self.sql {
                summary.push_str(" …");
                format!(
                    "\n\n{}\n",
                    sqlformat::format(
                        &self.sql,
                        &sqlformat::QueryParams::None,
                        sqlformat::FormatOptions::default()
                    )
                )
            } else {
                String::new()
            };

            let rows = self.rows;

            log::logger().log(
                &log::Record::builder()
                    .args(format_args!(
                        "{}; rows: {}, elapsed: {:.3?}{}",
                        summary, rows, elapsed, sql
                    ))
                    .level(lvl)
                    .module_path_static(Some("sqlx::query"))
                    .target("sqlx::query")
                    .build(),
            );
        }
    }
}

impl<'q> Drop for QueryLogger<'q> {
    fn drop(&mut self) {
        self.finish();
    }
}

fn parse_query_summary(sql: &str) -> String {
    // For now, just take the first 4 words
    sql.split_whitespace()
        .take(4)
        .collect::<Vec<&str>>()
        .join(" ")
}
