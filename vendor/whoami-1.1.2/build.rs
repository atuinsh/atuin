fn main() {
    #[cfg(windows)]
    println!(r"cargo:rustc-link-search=C:\windows\system32");
}
