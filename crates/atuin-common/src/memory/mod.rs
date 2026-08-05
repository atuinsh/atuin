// `secret_box`/`secret_arc` use raw pointers, `mprotect`, and manual
// `Send`/`Sync` impls to harden secret memory; opt them out of the crate-wide
// `deny(unsafe_code)`. The doc-overindent allow covers `secret_arc`'s fixed
// docblock and its platform bullet lists.
#[allow(unsafe_code)]
mod hardening;
#[allow(unsafe_code, clippy::doc_overindented_list_items)]
mod secret_arc;
#[allow(unsafe_code)]
mod secret_box;

pub use {
    secret_arc::SecretArc, secret_arc::SecretArcRead, secret_box::SecretBox,
    secret_box::SecretBoxRead,
};
