fn main() {
    println!("cargo:rerun-if-changed=src/db/postgres/migrations");
    println!("cargo:rerun-if-changed=src/db/sqlite/migrations");
    println!("cargo:rerun-if-changed=src/db/mysql/migrations");
}
