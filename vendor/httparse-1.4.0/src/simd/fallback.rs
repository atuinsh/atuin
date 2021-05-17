use ::iter::Bytes;

// Fallbacks that do nothing...

#[inline(always)]
pub fn match_uri_vectored(_: &mut Bytes) {}
#[inline(always)]
pub fn match_header_value_vectored(_: &mut Bytes) {}
