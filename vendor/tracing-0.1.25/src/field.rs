//! Structured data associated with `Span`s and `Event`s.
pub use tracing_core::field::*;

use crate::Metadata;

/// Trait implemented to allow a type to be used as a field key.
///
/// <div class="information">
///     <div class="tooltip ignore" style="">ⓘ<span class="tooltiptext">Note</span></div>
/// </div>
/// <div class="example-wrap" style="display:inline-block">
/// <pre class="ignore" style="white-space:normal;font:inherit;">
/// <strong>Note</strong>: Although this is implemented for both the
/// <a href="./struct.Field.html"><code>Field</code></a> type <em>and</em> any
/// type that can be borrowed as an <code>&str</code>, only <code>Field</code>
/// allows <em>O</em>(1) access.
/// Indexing a field with a string results in an iterative search that performs
/// string comparisons. Thus, if possible, once the key for a field is known, it
/// should be used whenever possible.
/// </pre>
pub trait AsField: crate::sealed::Sealed {
    /// Attempts to convert `&self` into a `Field` with the specified `metadata`.
    ///
    /// If `metadata` defines this field, then the field is returned. Otherwise,
    /// this returns `None`.
    fn as_field(&self, metadata: &Metadata<'_>) -> Option<Field>;
}

// ===== impl AsField =====

impl AsField for Field {
    #[inline]
    fn as_field(&self, metadata: &Metadata<'_>) -> Option<Field> {
        if self.callsite() == metadata.callsite() {
            Some(self.clone())
        } else {
            None
        }
    }
}

impl<'a> AsField for &'a Field {
    #[inline]
    fn as_field(&self, metadata: &Metadata<'_>) -> Option<Field> {
        if self.callsite() == metadata.callsite() {
            Some((*self).clone())
        } else {
            None
        }
    }
}

impl AsField for str {
    #[inline]
    fn as_field(&self, metadata: &Metadata<'_>) -> Option<Field> {
        metadata.fields().field(&self)
    }
}

impl crate::sealed::Sealed for Field {}
impl<'a> crate::sealed::Sealed for &'a Field {}
impl crate::sealed::Sealed for str {}
