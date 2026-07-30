use bstr::BString;

/// The outcome of rendering shell config (aliases or vars) into a shell's
/// syntax.
///
/// Rendering is best-effort: an item a shell cannot express — an invalid name
/// for the dialect, or a body with no representation — lands in `skipped`
/// rather than failing the whole render, so a partial-but-valid config is
/// always produced and the shell still starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    /// The config snippet to source, as bytes.
    pub script: BString,
    /// Items with no representation in this shell.
    pub skipped: Vec<Skipped>,
}

/// An item that could not be rendered, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub name: BString,
    pub reason: String,
}
