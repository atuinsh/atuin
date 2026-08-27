fn main() {
    // sqlx::migrate!() embeds these directories at compile time, but cargo only
    // reruns the macro when a tracked file changes - without this, adding a new
    // migration file does not trigger a recompile and silently ships a binary
    // missing the migration.
    println!("cargo:rerun-if-changed=migrations");
    println!("cargo:rerun-if-changed=meta-migrations");
    println!("cargo:rerun-if-changed=record-migrations");
}
