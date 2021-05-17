use std::env;
use std::fs;
use std::fmt::{Debug, Display};
use std::io;
use std::io::Write;
use std::path::Path;
use std::str;


/// Primary object used to write constant files.
/// 
/// # Example
/// ```no_run
/// # use std::path::Path;
/// # #[derive(Debug)]
/// # struct Point { x: u8, y: u8 }
/// use build_const::ConstWriter;
/// 
/// // use `for_build` in `build.rs`
/// let mut consts = ConstWriter::from_path(
///     &Path::new("/tmp/constants.rs")
/// ).unwrap();
/// 
/// // add an external dependency (`use xyz::Point`)
/// consts.add_dependency("xyz::Point");
/// 
/// // finish dependencies and starting writing constants
/// let mut consts = consts.finish_dependencies();
///
/// // add an array of values
/// let values: Vec<u8> = vec![1, 2, 3, 36];
/// consts.add_array("ARRAY", "u8", &values);
///
/// // Add a value that is a result of "complex" calculations
/// consts.add_value("VALUE", "u8", values.iter().sum::<u8>());
/// 
/// // Add a value from an external crate (must implement `Debug`)
/// consts.add_value("VALUE", "Point", &Point { x: 3, y: 7});
/// ```
pub struct ConstWriter {
    f: fs::File,
}

/// Created from `ConstWriter::finish_dependencies`. See
/// documentation for `ConstWriter`.
pub struct ConstValueWriter {
    f: fs::File,
}

impl ConstWriter {
    /// Create a ConstWriter to be used for your crate's `build.rs`
    pub fn for_build(mod_name: &str) -> io::Result<ConstWriter> {
        let out_dir = env::var("OUT_DIR").unwrap();
        let mod_name = format!("{}.rs", mod_name);
        let dest_path = Path::new(&out_dir).join(mod_name);

        Ok(ConstWriter {
            f: fs::File::create(&dest_path)?
        })
    }

    /// Create a new ConstWriter to write to an path. If a file
    /// already exists at the path then it will be deleted.
    pub fn from_path(path: &Path) -> io::Result<ConstWriter> {
        let f = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(path)?;
        Ok(ConstWriter {
            f: f,
        })
    }

    /// finish writing dependencies and start writing constants
    pub fn finish_dependencies(self) -> ConstValueWriter {
        ConstValueWriter { f: self.f }
    }

    /// Add a dependency to your constants file.
    pub fn add_dependency(&mut self, lib: &str) {
        write!(self.f, "pub use {};\n", lib).unwrap();
    }

    /// Add a raw string to the constants file.
    /// 
    /// This method only changes `raw` by adding a `\n` at the end.
    pub fn add_raw(&mut self, raw: &str) {
        write!(self.f, "{}\n", raw).unwrap();
    }

}

impl ConstValueWriter {
    /// Add a value to the constants file.
    /// 
    /// You have to manually specify the `name`, type (`ty`) and `value`
    /// of the constant you want to add.
    /// 
    /// The `value` uses the `Debug` trait to determine the formating of
    /// the value being added. If `Debug` is not accurate or will not work,
    /// you must use `add_value_raw` instead and format it yourself.
    pub fn add_value<T: Debug>(&mut self, name: &str, ty: &str, value: T) {
        self.add_value_raw(name, ty, &format!("{:?}", value));
    }

    /// Add a pre-formatted value to the constants file.
    /// 
    /// `add_value` depends on `Debug` being implemented in such a way
    /// that it accurately represents the type's creation. Sometimes that
    /// cannot be relied on and `add_value_raw` has to be used instead.
    pub fn add_value_raw(&mut self, name: &str, ty: &str, raw_value: &str) {
        write!(
            self.f, "pub const {}: {} = {};\n", 
            name, 
            ty,
            raw_value,
        ).unwrap();
    }

    /// Add an array of len > 0 to the constants
    /// 
    /// You have to manually specify the `name`, type (`ty`) of the **items** and 
    /// `values` of the array constant you want to add. The length of the array
    /// is determined automatically.
    /// 
    /// Example: `const.add_array("foo", "u16", &[1,2,3])`
    /// 
    /// The `value` of each item uses the `Debug` trait to determine the 
    /// formatting of the value being added. If `Debug` is not accurate or will 
    /// not work, you must use `add_array_raw` instead and format it yourself.
    pub fn add_array<T: Debug>(&mut self, name: &str, ty: &str, values: &[T]) {
        write_array(&mut self.f, name, ty, values);
    }

    /// Add an array of pre-formatted values to the constants file. The length of the array is
    /// determined automatically.
    /// 
    /// `add_array` depends on `Debug` being implemented for each item in such a way that it
    /// accurately represents the item's creation. Sometimes that cannot be relied on and
    /// `add_array_raw` has to be used instead.
    pub fn add_array_raw<S: AsRef<str> + Display>(&mut self, name: &str, ty: &str, raw_values: &[S]) {
        write_array_raw(&mut self.f, name, ty, raw_values);
    }

    /// Add a raw string to the constants file.
    /// 
    /// This method only changes `raw` by adding a `\n` at the end.
    pub fn add_raw(&mut self, raw: &str) {
        write!(self.f, "{}\n", raw).unwrap();
    }

    /// Finish writing to the constants file and consume self.
    pub fn finish(&mut self) {
        self.f.flush().unwrap();
    }

}

// Public Functions

/// Write an array and return the array's full type representation.
/// 
/// This can be used to create nested array constant types.
pub fn write_array<T: Debug, W: Write>(w: &mut W, name: &str, ty: &str, values: &[T]) 
    -> String
{
    assert!(
        !values.is_empty(), 
        "attempting to add an array of len zero. If this is intentional, use \
        add_value_raw instead."
    );
    let full_ty = write_array_header(w, name, ty, values.len());
    for v in values.iter() {
        write_array_item_raw(w, &format!("{:?}", v));
    }
    write_array_end(w);
    full_ty
}

/// Write an array of raw values and return the array's full type representation.
/// 
/// This can be used to create nested array constant types.
pub fn write_array_raw<W: Write, S: AsRef<str> + Display>(
        w: &mut W, name: &str, ty: &str, raw_values: &[S]
    ) 
    -> String
{
    assert!(
        !raw_values.is_empty(), 
        "attempting to add an array of len zero. If this is intentional, use \
        add_value_raw instead."
    );
    let full_ty = write_array_header(w, name, ty, raw_values.len());
    for v in raw_values {
        write_array_item_raw(w, v);
    }
    write_array_end(w);
    full_ty
}

// Helpers

/// Write the array header and return the array's full type.
fn write_array_header<W: Write>(w: &mut W, name: &str, ty: &str, len: usize) -> String {
    let full_ty = format!("[{}; {}]", ty, len);
    write!(w, "pub const {}: {} = [\n", name, &full_ty).unwrap();
    full_ty
}

fn write_array_item_raw<W: Write, S: AsRef<str> + Display>(w: &mut W, raw_item: S) {
    write!(w, "    {},\n", raw_item).unwrap()
}

fn write_array_end<W: Write>(w: &mut W) {
    write!(w, "];\n").unwrap();
}
