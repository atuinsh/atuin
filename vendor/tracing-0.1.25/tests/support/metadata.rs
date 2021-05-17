use std::fmt;
use tracing::Metadata;

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct Expect {
    pub name: Option<String>,
    pub level: Option<tracing::Level>,
    pub target: Option<String>,
}

impl Expect {
    pub(in crate::support) fn check(&self, actual: &Metadata<'_>, ctx: fmt::Arguments<'_>) {
        if let Some(ref expected_name) = self.name {
            let name = actual.name();
            assert!(
                expected_name == name,
                "expected {} to be named `{}`, but got one named `{}`",
                ctx,
                expected_name,
                name
            )
        }

        if let Some(ref expected_level) = self.level {
            let level = actual.level();
            assert!(
                expected_level == level,
                "expected {} to be at level `{:?}`, but it was at level `{:?}` instead",
                ctx,
                expected_level,
                level,
            )
        }

        if let Some(ref expected_target) = self.target {
            let target = actual.target();
            assert!(
                expected_target == target,
                "expected {} to have target `{}`, but it had target `{}` instead",
                ctx,
                expected_target,
                target,
            )
        }
    }
}

impl fmt::Display for Expect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref name) = self.name {
            write!(f, " named `{}`", name)?;
        }

        if let Some(ref level) = self.level {
            write!(f, " at the `{:?}` level", level)?;
        }

        if let Some(ref target) = self.target {
            write!(f, " with target `{}`", target)?;
        }

        Ok(())
    }
}
