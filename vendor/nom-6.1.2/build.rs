extern crate version_check;

fn main() {
  if version_check::is_min_version("1.44.0").unwrap_or(true) {
    println!("cargo:rustc-cfg=stable_i128");
  }
}
