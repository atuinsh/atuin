//! A stably addressed token buffer supporting efficient traversal based on a
//! cheaply copyable cursor.
//!
//! *This module is available only if Syn is built with the `"parsing"` feature.*

// This module is heavily commented as it contains most of the unsafe code in
// Syn, and caution should be used when editing it. The public-facing interface
// is 100% safe but the implementation is fragile internally.

#[cfg(all(
    not(all(target_arch = "wasm32", any(target_os = "unknown", target_os = "wasi"))),
    feature = "proc-macro"
))]
use crate::proc_macro as pm;
use crate::Lifetime;
use proc_macro2::{Delimiter, Group, Ident, Literal, Punct, Spacing, Span, TokenStream, TokenTree};
use std::marker::PhantomData;
use std::ptr;

/// Internal type which is used instead of `TokenTree` to represent a token tree
/// within a `TokenBuffer`.
enum Entry {
    // Mimicking types from proc-macro.
    Group(Group, TokenBuffer),
    Ident(Ident),
    Punct(Punct),
    Literal(Literal),
    // End entries contain a raw pointer to the entry from the containing
    // token tree, or null if this is the outermost level.
    End(*const Entry),
}

/// A buffer that can be efficiently traversed multiple times, unlike
/// `TokenStream` which requires a deep copy in order to traverse more than
/// once.
///
/// *This type is available only if Syn is built with the `"parsing"` feature.*
pub struct TokenBuffer {
    // NOTE: Do not derive clone on this - there are raw pointers inside which
    // will be messed up. Moving the `TokenBuffer` itself is safe as the actual
    // backing slices won't be moved.
    data: Box<[Entry]>,
}

impl TokenBuffer {
    // NOTE: DO NOT MUTATE THE `Vec` RETURNED FROM THIS FUNCTION ONCE IT
    // RETURNS, THE ADDRESS OF ITS BACKING MEMORY MUST REMAIN STABLE.
    fn inner_new(stream: TokenStream, up: *const Entry) -> TokenBuffer {
        // Build up the entries list, recording the locations of any Groups
        // in the list to be processed later.
        let mut entries = Vec::new();
        let mut seqs = Vec::new();
        for tt in stream {
            match tt {
                TokenTree::Ident(sym) => {
                    entries.push(Entry::Ident(sym));
                }
                TokenTree::Punct(op) => {
                    entries.push(Entry::Punct(op));
                }
                TokenTree::Literal(l) => {
                    entries.push(Entry::Literal(l));
                }
                TokenTree::Group(g) => {
                    // Record the index of the interesting entry, and store an
                    // `End(null)` there temporarially.
                    seqs.push((entries.len(), g));
                    entries.push(Entry::End(ptr::null()));
                }
            }
        }
        // Add an `End` entry to the end with a reference to the enclosing token
        // stream which was passed in.
        entries.push(Entry::End(up));

        // NOTE: This is done to ensure that we don't accidentally modify the
        // length of the backing buffer. The backing buffer must remain at a
        // constant address after this point, as we are going to store a raw
        // pointer into it.
        let mut entries = entries.into_boxed_slice();
        for (idx, group) in seqs {
            // We know that this index refers to one of the temporary
            // `End(null)` entries, and we know that the last entry is
            // `End(up)`, so the next index is also valid.
            let seq_up = &entries[idx + 1] as *const Entry;

            // The end entry stored at the end of this Entry::Group should
            // point to the Entry which follows the Group in the list.
            let inner = Self::inner_new(group.stream(), seq_up);
            entries[idx] = Entry::Group(group, inner);
        }

        TokenBuffer { data: entries }
    }

    /// Creates a `TokenBuffer` containing all the tokens from the input
    /// `TokenStream`.
    ///
    /// *This method is available only if Syn is built with both the `"parsing"` and
    /// `"proc-macro"` features.*
    #[cfg(all(
        not(all(target_arch = "wasm32", any(target_os = "unknown", target_os = "wasi"))),
        feature = "proc-macro"
    ))]
    pub fn new(stream: pm::TokenStream) -> TokenBuffer {
        Self::new2(stream.into())
    }

    /// Creates a `TokenBuffer` containing all the tokens from the input
    /// `TokenStream`.
    pub fn new2(stream: TokenStream) -> TokenBuffer {
        Self::inner_new(stream, ptr::null())
    }

    /// Creates a cursor referencing the first token in the buffer and able to
    /// traverse until the end of the buffer.
    pub fn begin(&self) -> Cursor {
        unsafe { Cursor::create(&self.data[0], &self.data[self.data.len() - 1]) }
    }
}

/// A cheaply copyable cursor into a `TokenBuffer`.
///
/// This cursor holds a shared reference into the immutable data which is used
/// internally to represent a `TokenStream`, and can be efficiently manipulated
/// and copied around.
///
/// An empty `Cursor` can be created directly, or one may create a `TokenBuffer`
/// object and get a cursor to its first token with `begin()`.
///
/// Two cursors are equal if they have the same location in the same input
/// stream, and have the same scope.
///
/// *This type is available only if Syn is built with the `"parsing"` feature.*
pub struct Cursor<'a> {
    // The current entry which the `Cursor` is pointing at.
    ptr: *const Entry,
    // This is the only `Entry::End(..)` object which this cursor is allowed to
    // point at. All other `End` objects are skipped over in `Cursor::create`.
    scope: *const Entry,
    // Cursor is covariant in 'a. This field ensures that our pointers are still
    // valid.
    marker: PhantomData<&'a Entry>,
}

impl<'a> Cursor<'a> {
    /// Creates a cursor referencing a static empty TokenStream.
    pub fn empty() -> Self {
        // It's safe in this situation for us to put an `Entry` object in global
        // storage, despite it not actually being safe to send across threads
        // (`Ident` is a reference into a thread-local table). This is because
        // this entry never includes a `Ident` object.
        //
        // This wrapper struct allows us to break the rules and put a `Sync`
        // object in global storage.
        struct UnsafeSyncEntry(Entry);
        unsafe impl Sync for UnsafeSyncEntry {}
        static EMPTY_ENTRY: UnsafeSyncEntry = UnsafeSyncEntry(Entry::End(0 as *const Entry));

        Cursor {
            ptr: &EMPTY_ENTRY.0,
            scope: &EMPTY_ENTRY.0,
            marker: PhantomData,
        }
    }

    /// This create method intelligently exits non-explicitly-entered
    /// `None`-delimited scopes when the cursor reaches the end of them,
    /// allowing for them to be treated transparently.
    unsafe fn create(mut ptr: *const Entry, scope: *const Entry) -> Self {
        // NOTE: If we're looking at a `End(..)`, we want to advance the cursor
        // past it, unless `ptr == scope`, which means that we're at the edge of
        // our cursor's scope. We should only have `ptr != scope` at the exit
        // from None-delimited groups entered with `ignore_none`.
        while let Entry::End(exit) = *ptr {
            if ptr == scope {
                break;
            }
            ptr = exit;
        }

        Cursor {
            ptr,
            scope,
            marker: PhantomData,
        }
    }

    /// Get the current entry.
    fn entry(self) -> &'a Entry {
        unsafe { &*self.ptr }
    }

    /// Bump the cursor to point at the next token after the current one. This
    /// is undefined behavior if the cursor is currently looking at an
    /// `Entry::End`.
    unsafe fn bump(self) -> Cursor<'a> {
        Cursor::create(self.ptr.offset(1), self.scope)
    }

    /// While the cursor is looking at a `None`-delimited group, move it to look
    /// at the first token inside instead. If the group is empty, this will move
    /// the cursor past the `None`-delimited group.
    ///
    /// WARNING: This mutates its argument.
    fn ignore_none(&mut self) {
        while let Entry::Group(group, buf) = self.entry() {
            if group.delimiter() == Delimiter::None {
                // NOTE: We call `Cursor::create` here to make sure that
                // situations where we should immediately exit the span after
                // entering it are handled correctly.
                unsafe {
                    *self = Cursor::create(&buf.data[0], self.scope);
                }
            } else {
                break;
            }
        }
    }

    /// Checks whether the cursor is currently pointing at the end of its valid
    /// scope.
    pub fn eof(self) -> bool {
        // We're at eof if we're at the end of our scope.
        self.ptr == self.scope
    }

    /// If the cursor is pointing at a `Group` with the given delimiter, returns
    /// a cursor into that group and one pointing to the next `TokenTree`.
    pub fn group(mut self, delim: Delimiter) -> Option<(Cursor<'a>, Span, Cursor<'a>)> {
        // If we're not trying to enter a none-delimited group, we want to
        // ignore them. We have to make sure to _not_ ignore them when we want
        // to enter them, of course. For obvious reasons.
        if delim != Delimiter::None {
            self.ignore_none();
        }

        if let Entry::Group(group, buf) = self.entry() {
            if group.delimiter() == delim {
                return Some((buf.begin(), group.span(), unsafe { self.bump() }));
            }
        }

        None
    }

    /// If the cursor is pointing at a `Ident`, returns it along with a cursor
    /// pointing at the next `TokenTree`.
    pub fn ident(mut self) -> Option<(Ident, Cursor<'a>)> {
        self.ignore_none();
        match self.entry() {
            Entry::Ident(ident) => Some((ident.clone(), unsafe { self.bump() })),
            _ => None,
        }
    }

    /// If the cursor is pointing at an `Punct`, returns it along with a cursor
    /// pointing at the next `TokenTree`.
    pub fn punct(mut self) -> Option<(Punct, Cursor<'a>)> {
        self.ignore_none();
        match self.entry() {
            Entry::Punct(op) if op.as_char() != '\'' => Some((op.clone(), unsafe { self.bump() })),
            _ => None,
        }
    }

    /// If the cursor is pointing at a `Literal`, return it along with a cursor
    /// pointing at the next `TokenTree`.
    pub fn literal(mut self) -> Option<(Literal, Cursor<'a>)> {
        self.ignore_none();
        match self.entry() {
            Entry::Literal(lit) => Some((lit.clone(), unsafe { self.bump() })),
            _ => None,
        }
    }

    /// If the cursor is pointing at a `Lifetime`, returns it along with a
    /// cursor pointing at the next `TokenTree`.
    pub fn lifetime(mut self) -> Option<(Lifetime, Cursor<'a>)> {
        self.ignore_none();
        match self.entry() {
            Entry::Punct(op) if op.as_char() == '\'' && op.spacing() == Spacing::Joint => {
                let next = unsafe { self.bump() };
                match next.ident() {
                    Some((ident, rest)) => {
                        let lifetime = Lifetime {
                            apostrophe: op.span(),
                            ident,
                        };
                        Some((lifetime, rest))
                    }
                    None => None,
                }
            }
            _ => None,
        }
    }

    /// Copies all remaining tokens visible from this cursor into a
    /// `TokenStream`.
    pub fn token_stream(self) -> TokenStream {
        let mut tts = Vec::new();
        let mut cursor = self;
        while let Some((tt, rest)) = cursor.token_tree() {
            tts.push(tt);
            cursor = rest;
        }
        tts.into_iter().collect()
    }

    /// If the cursor is pointing at a `TokenTree`, returns it along with a
    /// cursor pointing at the next `TokenTree`.
    ///
    /// Returns `None` if the cursor has reached the end of its stream.
    ///
    /// This method does not treat `None`-delimited groups as transparent, and
    /// will return a `Group(None, ..)` if the cursor is looking at one.
    pub fn token_tree(self) -> Option<(TokenTree, Cursor<'a>)> {
        let tree = match self.entry() {
            Entry::Group(group, _) => group.clone().into(),
            Entry::Literal(lit) => lit.clone().into(),
            Entry::Ident(ident) => ident.clone().into(),
            Entry::Punct(op) => op.clone().into(),
            Entry::End(..) => {
                return None;
            }
        };

        Some((tree, unsafe { self.bump() }))
    }

    /// Returns the `Span` of the current token, or `Span::call_site()` if this
    /// cursor points to eof.
    pub fn span(self) -> Span {
        match self.entry() {
            Entry::Group(group, _) => group.span(),
            Entry::Literal(l) => l.span(),
            Entry::Ident(t) => t.span(),
            Entry::Punct(o) => o.span(),
            Entry::End(..) => Span::call_site(),
        }
    }

    /// Skip over the next token without cloning it. Returns `None` if this
    /// cursor points to eof.
    ///
    /// This method treats `'lifetimes` as a single token.
    pub(crate) fn skip(self) -> Option<Cursor<'a>> {
        match self.entry() {
            Entry::End(..) => None,

            // Treat lifetimes as a single tt for the purposes of 'skip'.
            Entry::Punct(op) if op.as_char() == '\'' && op.spacing() == Spacing::Joint => {
                let next = unsafe { self.bump() };
                match next.entry() {
                    Entry::Ident(_) => Some(unsafe { next.bump() }),
                    _ => Some(next),
                }
            }
            _ => Some(unsafe { self.bump() }),
        }
    }
}

impl<'a> Copy for Cursor<'a> {}

impl<'a> Clone for Cursor<'a> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a> Eq for Cursor<'a> {}

impl<'a> PartialEq for Cursor<'a> {
    fn eq(&self, other: &Self) -> bool {
        let Cursor { ptr, scope, marker } = self;
        let _ = marker;
        *ptr == other.ptr && *scope == other.scope
    }
}

pub(crate) fn same_scope(a: Cursor, b: Cursor) -> bool {
    a.scope == b.scope
}

pub(crate) fn open_span_of_group(cursor: Cursor) -> Span {
    match cursor.entry() {
        Entry::Group(group, _) => group.span_open(),
        _ => cursor.span(),
    }
}

pub(crate) fn close_span_of_group(cursor: Cursor) -> Span {
    match cursor.entry() {
        Entry::Group(group, _) => group.span_close(),
        _ => cursor.span(),
    }
}
