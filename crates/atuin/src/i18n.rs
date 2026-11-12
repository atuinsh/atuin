//! Localization of user-facing text.
//!
//! Messages are Fluent strings in `i18n/<language>/atuin.ftl`, looked up by id with [`fl!`]. `fl!`
//! fails the build if an id is missing from the `en-US` catalogue or its arguments don't match the
//! message's variables.

atuin_common::i18n::catalogue!();

/// Look up a message in the user's language: `fl!("message-id", name = value)`, or one of its
/// attributes: `fl!("message-id", "long")`.
macro_rules! fl {
    ($($args:tt)+) => {
        atuin_common::i18n::fl!($crate::i18n::LOADER; $($args)+)
    };
}

/// [`write!`](std::write) a message looked up with [`fl!`].
macro_rules! writet {
    ($dst:expr, $($args:tt)+) => {
        ::std::write!($dst, "{}", $crate::i18n::fl!($($args)+))
    };
}

pub(crate) use fl;
pub(crate) use writet;
