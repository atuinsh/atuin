fn main() {
    for dir in ["src/db/postgres/migrations", "src/db/sqlite/migrations", "src/db/mysql/migrations"]
    {
        build_rs::output::rerun_if_changed(dir);
    }
}
