fn main() {
    // sqlx::migrate!() embeds these directories at compile time, but cargo only
    // reruns the macro when a tracked file changes - without this, adding a new
    // migration file does not trigger a recompile and silently ships a binary
    // missing the migration.
    for dir in ["migrations", "meta-migrations", "record-migrations", "src/ai_session/migrations"] {
        build_rs::output::rerun_if_changed(dir);
    }
}
