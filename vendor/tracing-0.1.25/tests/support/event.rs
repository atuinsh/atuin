#![allow(missing_docs)]
use super::{field, metadata, Parent};

use std::fmt;

/// A mock event.
///
/// This is intended for use with the mock subscriber API in the
/// `subscriber` module.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct MockEvent {
    pub fields: Option<field::Expect>,
    pub(in crate::support) parent: Option<Parent>,
    metadata: metadata::Expect,
}

pub fn mock() -> MockEvent {
    MockEvent {
        ..Default::default()
    }
}

impl MockEvent {
    pub fn named<I>(self, name: I) -> Self
    where
        I: Into<String>,
    {
        Self {
            metadata: metadata::Expect {
                name: Some(name.into()),
                ..self.metadata
            },
            ..self
        }
    }

    pub fn with_fields<I>(self, fields: I) -> Self
    where
        I: Into<field::Expect>,
    {
        Self {
            fields: Some(fields.into()),
            ..self
        }
    }

    pub fn at_level(self, level: tracing::Level) -> Self {
        Self {
            metadata: metadata::Expect {
                level: Some(level),
                ..self.metadata
            },
            ..self
        }
    }

    pub fn with_target<I>(self, target: I) -> Self
    where
        I: Into<String>,
    {
        Self {
            metadata: metadata::Expect {
                target: Some(target.into()),
                ..self.metadata
            },
            ..self
        }
    }

    pub fn with_explicit_parent(self, parent: Option<&str>) -> MockEvent {
        let parent = match parent {
            Some(name) => Parent::Explicit(name.into()),
            None => Parent::ExplicitRoot,
        };
        Self {
            parent: Some(parent),
            ..self
        }
    }

    pub(in crate::support) fn check(&mut self, event: &tracing::Event<'_>) {
        let meta = event.metadata();
        let name = meta.name();
        self.metadata
            .check(meta, format_args!("event \"{}\"", name));
        assert!(meta.is_event(), "expected {}, but got {:?}", self, event);
        if let Some(ref mut expected_fields) = self.fields {
            let mut checker = expected_fields.checker(name.to_string());
            event.record(&mut checker);
            checker.finish();
        }
    }
}

impl fmt::Display for MockEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "an event{}", self.metadata)
    }
}
