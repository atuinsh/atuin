#![allow(dead_code)]
pub mod event;
pub mod field;
mod metadata;
pub mod span;
pub mod subscriber;

#[derive(Debug, Eq, PartialEq)]
pub(in crate::support) enum Parent {
    ContextualRoot,
    Contextual(String),
    ExplicitRoot,
    Explicit(String),
}
