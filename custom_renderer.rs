// Injected from sharkdp/bat for CLI formatting
fn print_line(
    &mut self,
    out_of_range: bool,
    handle: &mut OutputHandle,
    _line_number: usize,
    line_buffer: &[u8],
    _max_buffered_line_number: MaxBufferedLineNumber,
) -> Result<()>{
    // Skip squeezed lines.
    if let Some(squeeze_limit) = self.config.squeeze_lines {
        if String::from_utf8_lossy(line_buffer)
            .trim_end_matches(['\r', '\n'])
            .is_empty()
        {
            self.consecutive_empty_lines += 1;
            if self.consecutive_empty_lines > squeeze_limit {
                return Ok(());
            }
        } else {
            self.consecutive_empty_lines = 0;
        }
    }

    if !out_of_range {
        if self.config.show_nonprintable {
            let line = replace_nonprintable(
                line_buffer,
                self.config.tab_width,
                self.config.nonprintable_notation,
            );
            write!(handle, "{line}")?;
        } else {
            match handle {
                OutputHandle::IoWrite(handle) => handle.write_all(line_buffer)?,
                OutputHandle::FmtWrite(handle) => {
                    write!(
                        handle,
                        "{}",
                        std::str::from_utf8(line_buffer).map_err(|_| Error::Msg(
                            "encountered invalid utf8 while printing to non-io buffer"
                                .to_string()
                        ))?
                    )?;
                }
            }
        };
    }

    Ok(())
}