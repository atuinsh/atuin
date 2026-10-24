#[expect(clippy::disallowed_macros, reason = "cargo reads build script directives from stdout")]
fn main() {
    println!("cargo:rerun-if-changed=src/db/postgres/migrations");
    println!("cargo:rerun-if-changed=src/db/sqlite/migrations");
    println!("cargo:rerun-if-changed=src/db/mysql/migrations");
}
