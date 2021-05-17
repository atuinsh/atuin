use std::env;
use std::fs;
use std::iter;
use std::path::{self, Path};

/*
#[doc(hidden)]
#[macro_export]
macro_rules! count {
    () => { proc_macro_call_0!() };
    (!) => { proc_macro_call_1!() };
    (!!) => { proc_macro_call_2!() };
    ...
}
*/

fn main() {
    // Tell Cargo not to rerun on src/lib.rs changes.
    println!("cargo:rerun-if-changed=build.rs");

    let mut content = String::new();
    content += "#[doc(hidden)]\n";
    content += "#[macro_export]\n";
    content += "macro_rules! count {\n";
    for i in 0..=64 {
        let bangs = iter::repeat("!").take(i).collect::<String>();
        content += &format!("    ({}) => {{ proc_macro_call_{}!() }};\n", bangs, i);
    }
    content += "    ($(!)+) => {\n";
    content += "        compile_error! {\n";
    content += "            \"this macro does not support >64 nested macro invocations\"\n";
    content += "        }\n";
    content += "    };\n";
    content += "}\n";

    let content = content.as_bytes();
    let out_dir = env::var("OUT_DIR").unwrap();
    let ref dest_path = Path::new(&out_dir).join("count.rs");

    // Avoid bumping filetime if content is up to date. Possibly related to
    // https://github.com/dtolnay/proc-macro-hack/issues/56 ...?
    if fs::read(dest_path)
        .map(|existing| existing != content)
        .unwrap_or(true)
    {
        fs::write(dest_path, content).unwrap();
    }

    println!("cargo:rustc-env=PATH_SEPARATOR={}", path::MAIN_SEPARATOR);
}
