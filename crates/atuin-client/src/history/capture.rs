/// A completed command's captured output.
///
/// This is the domain representation of a captured command, independent of any wire (gRPC) or
/// storage format. The daemon persists this type directly; the gRPC layer converts to and from its
/// own protobuf types at the edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCapture {
    /// The rendered output of the command.
    ///
    /// Contains SGR escape sequences. Contains no other escape sequences, and no control characters
    /// except `'\n'`.
    pub output: String,
    /// The total number of bytes that were pushed to the virtual terminal.
    ///
    /// This counts bytes observed *before* rasterizing the terminal, so it is not the same as
    /// `output.len()`.
    pub output_observed_bytes: u64,
    /// Whether [`Self::output`] was truncated because it would have exceeded a maximal limit.
    pub output_truncated: bool,
    /// The width of the terminal when the command finished.
    pub terminal_width: u16,
    /// The height of the terminal when the command finished.
    pub terminal_height: u16,
}
