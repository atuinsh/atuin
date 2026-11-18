//! The organizations a config file names.
//!
//! Lua shares one string between all the entries that hold it, but Rust copies it into each. So a
//! list and each slug in it are bounded before they're copied. Otherwise a file could repeat one
//! long string to make atuin allocate far past the VM's memory limit.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::RangeInclusive;
use std::str::FromStr;

use derive_more::{AsRef, Deref, Display};
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_with::DeserializeFromStr;
use thiserror::Error;

/// The organizations a file names: at most [`Orgs::MAX`] of them.
#[derive(Debug, Clone, PartialEq, Eq, Deref)]
pub struct Orgs(Vec<OrgSlug>);

impl Orgs {
    /// The most a file may list. A project belongs to one organization, or to a few.
    pub const MAX: usize = 64;
}

impl TryFrom<Vec<OrgSlug>> for Orgs {
    type Error = TooManyOrgs;

    fn try_from(orgs: Vec<OrgSlug>) -> Result<Self, Self::Error> {
        if orgs.len() > Self::MAX {
            return Err(TooManyOrgs);
        }
        Ok(Self(orgs))
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("more than {} organizations", Orgs::MAX)]
pub struct TooManyOrgs;

impl<'de> Deserialize<'de> for Orgs {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_seq(OrgsVisitor)
    }
}

struct OrgsVisitor;

impl<'de> Visitor<'de> for OrgsVisitor {
    type Value = Orgs;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a list of at most {} organizations", Orgs::MAX)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Orgs, A::Error> {
        let mut orgs = Vec::new();
        while let Some(org) = seq.next_element()? {
            if orgs.len() == Orgs::MAX {
                return Err(de::Error::custom(TooManyOrgs));
            }
            orgs.push(org);
        }
        Ok(Orgs(orgs))
    }
}

/// An organization's slug, as the hub takes one: 2 to 32 letters, digits, `-` and `_`. It doesn't
/// start or end with `-`, or hold `--`. Slugs are equal whatever their case, as the hub's are.
#[derive(Debug, Clone, AsRef, Display, DeserializeFromStr)]
#[as_ref(str)]
pub struct OrgSlug(String);

impl PartialEq for OrgSlug {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl Eq for OrgSlug {}

impl Hash for OrgSlug {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for byte in self.0.bytes() {
            state.write_u8(byte.to_ascii_lowercase());
        }
    }
}

impl OrgSlug {
    /// The hub's limits, from `api/lib/atuin_hub/orgs/organization.ex`.
    const LEN: RangeInclusive<usize> = 2..=32;
}

impl FromStr for OrgSlug {
    type Err = OrgSlugError;

    fn from_str(slug: &str) -> Result<Self, Self::Err> {
        // The length goes first, so that a long string is never copied into the error.
        if !Self::LEN.contains(&slug.len()) {
            return Err(OrgSlugError::Length(slug.len()));
        }
        let allowed = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if !slug.chars().all(allowed)
            || slug.starts_with('-')
            || slug.ends_with('-')
            || slug.contains("--")
        {
            return Err(OrgSlugError::Format(slug.to_owned()));
        }
        Ok(Self(slug.to_owned()))
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OrgSlugError {
    #[error("an organization's slug is 2 to 32 characters, not {0} bytes")]
    Length(usize),
    #[error(
        "{0:?} isn't an organization's slug: it may hold only letters, digits, `-` and `_`, and \
         may not start or end with `-` or hold `--`"
    )]
    Format(String),
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::iter;

    use proptest::prelude::*;
    use regex::Regex;
    use rstest::rstest;
    use serde::de::value::{self, SeqDeserializer};

    use super::*;

    /// The hub's own check of a slug, as it's written in `organization.ex`.
    fn hub_takes(slug: &str) -> bool {
        let format = Regex::new(r"\A[A-Za-z0-9_][A-Za-z0-9_-]*[A-Za-z0-9_]\z").expect("a regex");
        (2..=32).contains(&slug.chars().count()) && format.is_match(slug) && !slug.contains("--")
    }

    fn orgs<'a>(slugs: impl IntoIterator<Item = &'a str>) -> Result<Orgs, value::Error> {
        Orgs::deserialize(SeqDeserializer::new(slugs.into_iter()))
    }

    proptest! {
        /// The hub decides which slugs exist, so a file may name an org by any slug the hub takes,
        /// and by no other.
        #[test]
        fn a_slug_is_valid_when_the_hub_takes_it(slug in "[a-zA-Z0-9_ .ü-]{0,40}") {
            prop_assert_eq!(slug.parse::<OrgSlug>().is_ok(), hub_takes(&slug));
        }
    }

    #[rstest]
    #[case::empty("", OrgSlugError::Length(0))]
    #[case::too_long("abcdefghijklmnopqrstuvwxyz0123456", OrgSlugError::Length(33))]
    #[case::leading_dash("-acme", OrgSlugError::Format("-acme".to_owned()))]
    #[case::double_dash("ac--me", OrgSlugError::Format("ac--me".to_owned()))]
    #[case::not_ascii("acmé", OrgSlugError::Format("acmé".to_owned()))]
    fn a_slug_the_hub_wouldnt_take_says_why(#[case] slug: &str, #[case] expected: OrgSlugError) {
        assert_eq!(slug.parse::<OrgSlug>(), Err(expected));
    }

    #[rstest]
    #[case::same("acme", "acme", true)]
    #[case::cased("acme", "ACMe", true)]
    #[case::other("acme", "globex", false)]
    fn slugs_are_equal_whatever_their_case(#[case] a: &str, #[case] b: &str, #[case] equal: bool) {
        let (a, b): (OrgSlug, OrgSlug) = (a.parse().expect("a slug"), b.parse().expect("a slug"));

        assert_eq!(a == b, equal);
        assert_eq!(HashSet::from([a]).contains(&b), equal);
    }

    #[rstest]
    #[case::none(0)]
    #[case::at_the_limit(Orgs::MAX)]
    fn a_list_up_to_the_limit_is_valid(#[case] count: usize) {
        let orgs = orgs(iter::repeat_n("acme", count)).expect("orgs");

        assert_eq!(orgs.len(), count);
    }

    #[rstest]
    fn a_list_past_the_limit_is_invalid() {
        let err = orgs(iter::repeat_n("acme", Orgs::MAX + 1)).expect_err("too many orgs");

        assert_eq!(err.to_string(), "more than 64 organizations");
    }

    /// Orgs built in Rust, as `atuin workspace init` builds them, hold to the limit a file does.
    #[rstest]
    #[case::at_the_limit(Orgs::MAX, Ok(Orgs::MAX))]
    #[case::past_the_limit(Orgs::MAX + 1, Err(TooManyOrgs))]
    fn orgs_built_from_slugs_keep_to_the_limit(
        #[case] count: usize,
        #[case] expected: Result<usize, TooManyOrgs>,
    ) {
        let slug: OrgSlug = "acme".parse().expect("a slug");

        let orgs = Orgs::try_from(vec![slug; count]);

        assert_eq!(orgs.map(|orgs| orgs.len()), expected);
    }
}
