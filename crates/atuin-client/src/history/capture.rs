use vt100::capture::basic_formatted_to_plain;

/// A completed command's captured output.
///
/// This is the domain representation of a captured command, independent of any wire (gRPC) or
/// storage format. The daemon persists this type directly; the gRPC layer converts to and from its
/// own protobuf types at the edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCapture {
    /// The starting portion of the rendered output of the command.
    ///
    /// Contains SGR escape sequences. Contains no other escape sequences, and no control characters
    /// except `'\n'`.
    ///
    /// If `output_end` is [`None`], the output didn't need to be truncated, so `output_start`
    /// contains the full output.
    pub output_start: String,

    /// The ending portion of the rendered output of the command.
    ///
    /// If this is [`Some`], the middle portion of the output had to be discarded because the output
    /// exceeded the size limit.
    pub output_end: Option<String>,

    /// The total number of bytes that were pushed to the virtual terminal.
    ///
    /// This counts bytes observed *before* rasterizing the terminal, so it is not the same as
    /// `output.len()`.
    pub output_observed_bytes: u64,

    /// The width of the terminal when the command finished.
    pub terminal_width: u16,

    /// The height of the terminal when the command finished.
    pub terminal_height: u16,
}

impl CommandCapture {
    /// Write the command capture as a plaintext string, stripping away all escape codes.
    #[must_use]
    pub fn plaintext(&self) -> String {
        let mut out = String::with_capacity(
            self.output_start.len() + self.output_end.as_ref().map_or(0, |end| end.len() + 1),
        );
        out.extend(basic_formatted_to_plain(&self.output_start));

        if let Some(end) = &self.output_end {
            out.push('\n');
            out.extend(basic_formatted_to_plain(end));
        }
        out
    }
}
