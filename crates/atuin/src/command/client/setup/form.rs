//! A form of choices that depend on each other, each one set on the screen or on the command line.
//!
//! Each field of a [`Form`] is a type implementing [`Choice`], which says whether the form asks
//! about it and what it needs from the rest of the form, and [`Flag`], which says how the command
//! line sets it. The form keeps what was chosen. [`Form::effective`] is what to set up: each field
//! the rest rules out at its default, which is off, and each field the form doesn't ask about as
//! it was given.

use clap::{ArgMatches, Command};

/// How the command line sets a field.
pub(super) trait Flag: Sized {
    /// Adds the field's flags to `cmd`.
    fn args(cmd: Command) -> Command;

    /// The field as the flags in `matches` set it, `None` when none does.
    fn from_args(matches: &ArgMatches) -> Option<Self>;

    /// The flag that sets the field as it is, as `--ctrl-r`.
    fn flag(&self) -> String;
}

/// A field of the form `F`. Its default is what it is while the rest of the form rules it out.
pub(super) trait Choice<F>: Clone + PartialEq + Default + Flag {
    /// Whether `form` asks about the field, or why it doesn't. A field it doesn't ask about keeps
    /// the value it was given.
    ///
    /// Read what the form was made with, not its other fields: what a form asks is fixed.
    fn asked(_form: &F) -> Result<(), &'static str> {
        Ok(())
    }

    /// Whether the rest of `form` allows the field, or what it needs.
    fn allowed(_form: &F) -> Result<(), Needs> {
        Ok(())
    }
}

/// What a field needs from the rest of its form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Needs {
    /// What it needs, as the screen says it: `shell history`.
    pub(super) what: &'static str,
    /// The flags that give it: `` `--shell-history` ``.
    pub(super) flags: &'static str,
}

/// A flag that turns on a field the form can't have on.
#[derive(Debug, thiserror::Error)]
pub(super) enum FlagError {
    #[error("`{flag}` can't be on here: {why}")]
    NotAsked {
        flag: String,
        why: &'static str,
    },
    #[error("`{flag}` needs {}: {}", .needs.what, .needs.flags)]
    Needs {
        flag: String,
        needs: Needs,
    },
}

/// A struct of [`Choice`]s.
pub(super) trait Form: Clone + Default {
    /// Hands each field to `visit`.
    fn visit(&mut self, visit: &mut impl Visit<Self>);

    /// The form as it's set up: each field the rest rules out at its default.
    ///
    /// `self` keeps what was chosen, so turning a field back on brings back the fields that
    /// depend on it as they were.
    #[must_use]
    fn effective(&self) -> Self {
        let mut form = self.clone();
        // A field ruled out goes to its default and stays there, so this settles within a pass per
        // field, whatever order they're visited in.
        loop {
            let before = form.clone();
            let mut rule_out = RuleOut {
                form: &before,
                changed: false,
            };
            form.visit(&mut rule_out);
            if !rule_out.changed {
                return form;
            }
        }
    }

    /// Whether the rest of the form allows the field `C`, or what it needs: `None` when the form
    /// doesn't ask about it.
    fn ask<C: Choice<Self>>(&self) -> Option<Result<(), Needs>> {
        C::asked(self).ok()?;
        Some(C::allowed(&self.effective()))
    }

    /// Adds every field's flags to `cmd`.
    fn args(cmd: Command) -> Command {
        let mut add = AddArgs(cmd);
        Self::default().visit(&mut add);
        add.0
    }

    /// Sets each field that the flags in `matches` set.
    fn set_from(&mut self, matches: &ArgMatches) {
        self.visit(&mut SetFrom(matches));
    }

    /// Fails on the first flag in `matches` that turns on a field the form can't have on. Flags
    /// that turn a field off never fail.
    fn check(&self, matches: &ArgMatches) -> Result<(), FlagError> {
        let effective = self.effective();
        let mut check = Check {
            form: &effective,
            matches,
            error: None,
        };
        effective.clone().visit(&mut check);
        check.error.map_or(Ok(()), Err)
    }
}

/// Something done to each field of a form.
pub(super) trait Visit<F> {
    fn field<C: Choice<F>>(&mut self, field: &mut C);
}

/// Sets each field `form` asks about but rules out to its default.
struct RuleOut<'a, F> {
    form: &'a F,
    changed: bool,
}

impl<F> Visit<F> for RuleOut<'_, F> {
    fn field<C: Choice<F>>(&mut self, field: &mut C) {
        if C::asked(self.form).is_ok() && C::allowed(self.form).is_err() && *field != C::default() {
            *field = C::default();
            self.changed = true;
        }
    }
}

struct AddArgs(Command);

impl<F> Visit<F> for AddArgs {
    fn field<C: Choice<F>>(&mut self, _: &mut C) {
        self.0 = C::args(std::mem::take(&mut self.0));
    }
}

struct SetFrom<'a>(&'a ArgMatches);

impl<F> Visit<F> for SetFrom<'_> {
    fn field<C: Choice<F>>(&mut self, field: &mut C) {
        if let Some(value) = C::from_args(self.0) {
            *field = value;
        }
    }
}

/// Finds the first field of the effective `form` that a flag in `matches` turned on in vain.
struct Check<'a, F> {
    form: &'a F,
    matches: &'a ArgMatches,
    error: Option<FlagError>,
}

impl<F> Visit<F> for Check<'_, F> {
    fn field<C: Choice<F>>(&mut self, _: &mut C) {
        let Some(flagged) = C::from_args(self.matches) else {
            return;
        };
        if self.error.is_some() || flagged == C::default() {
            return;
        }
        let flag = flagged.flag();
        self.error = match (C::asked(self.form), C::allowed(self.form)) {
            (Err(why), _) => Some(FlagError::NotAsked { flag, why }),
            (Ok(()), Err(needs)) => Some(FlagError::Needs { flag, needs }),
            (Ok(()), Ok(())) => None,
        };
    }
}

/// Declares choices that are on or off, each set by `--<flag>` and `--no-<flag>`, whichever comes
/// last. Short help shows only `--<flag>`; full help, which `install.sh` reads, shows both. An
/// `alias` is a hidden old name for `--<flag>`.
macro_rules! toggles {
    ($(
        $(#[$attr:meta])*
        $name:ident(
            $flag:literal $(, $heading:literal)? $(; hidden = $hidden:literal)?
            $(; alias = $alias:literal)?
        ) {
            on: $on:literal,
            off: $off:literal $(,)?
        }
    )*) => {$(
        $(#[$attr])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub(super) struct $name(pub(super) bool);

        impl $crate::command::client::setup::form::Flag for $name {
            fn args(cmd: ::clap::Command) -> ::clap::Command {
                const NO: &str = concat!("no-", $flag);
                cmd.arg(
                    ::clap::Arg::new($flag)
                        .long($flag)
                        .action(::clap::ArgAction::SetTrue)
                        .overrides_with_all([$flag, NO])
                        .help(concat!($on, " [or --no-", $flag, "]"))
                        .long_help($on)
                        $(.help_heading($heading))?
                        $(.hide($hidden))?
                        $(.alias($alias))?,
                )
                .arg(
                    ::clap::Arg::new(NO)
                        .long(NO)
                        .action(::clap::ArgAction::SetTrue)
                        .overrides_with_all([$flag, NO])
                        .help($off)
                        .hide_short_help(true)
                        $(.help_heading($heading))?
                        $(.hide($hidden))?,
                )
            }

            fn from_args(matches: &::clap::ArgMatches) -> Option<Self> {
                if matches.get_flag($flag) {
                    Some(Self(true))
                } else if matches.get_flag(concat!("no-", $flag)) {
                    Some(Self(false))
                } else {
                    None
                }
            }

            fn flag(&self) -> String {
                let flag = if self.0 { concat!("--", $flag) } else { concat!("--no-", $flag) };
                flag.to_owned()
            }
        }
    )*};
}

pub(super) use toggles;

#[cfg(test)]
mod tests {
    use clap::{ArgMatches, Command};
    use rstest::rstest;

    use super::*;

    toggles! {
        A("a") { on: "a", off: "not a" }
        /// Needs `A`.
        B("b") { on: "b", off: "not b" }
        /// Needs `B`.
        C("c") { on: "c", off: "not c" }
        /// Needs `A`, but isn't asked about.
        Unasked("unasked") { on: "unasked", off: "not unasked" }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct Test {
        a: A,
        b: B,
        c: C,
        unasked: Unasked,
    }

    impl Form for Test {
        // Each field before the one it needs, so ruling one out takes more than a pass.
        fn visit(&mut self, visit: &mut impl Visit<Self>) {
            visit.field(&mut self.unasked);
            visit.field(&mut self.c);
            visit.field(&mut self.b);
            visit.field(&mut self.a);
        }
    }

    impl Choice<Test> for A {}

    const NEEDS_A: Needs = Needs {
        what: "a",
        flags: "`--a`",
    };

    const NEEDS_B: Needs = Needs {
        what: "b",
        flags: "`--b`",
    };

    impl Choice<Test> for B {
        fn allowed(form: &Test) -> Result<(), Needs> {
            if form.a.0 {
                Ok(())
            } else {
                Err(NEEDS_A)
            }
        }
    }

    impl Choice<Test> for C {
        fn allowed(form: &Test) -> Result<(), Needs> {
            if form.b.0 {
                Ok(())
            } else {
                Err(NEEDS_B)
            }
        }
    }

    impl Choice<Test> for Unasked {
        fn asked(_: &Test) -> Result<(), &'static str> {
            Err("never")
        }

        fn allowed(form: &Test) -> Result<(), Needs> {
            if form.a.0 {
                Ok(())
            } else {
                Err(NEEDS_A)
            }
        }
    }

    /// The form with each field as given, in the order `a`, `b`, `c`, `unasked`.
    const fn test([a, b, c, unasked]: [bool; 4]) -> Test {
        Test {
            a: A(a),
            b: B(b),
            c: C(c),
            unasked: Unasked(unasked),
        }
    }

    fn matches(args: &[&str]) -> ArgMatches {
        let cmd = Test::args(Command::new("test"));
        cmd.try_get_matches_from(std::iter::once("test").chain(args.iter().copied())).unwrap()
    }

    /// The form all on, with `args` set on it, and what checking them says.
    fn flagged(args: &[&str]) -> (Test, Result<(), String>) {
        let matches = matches(args);
        let mut form = test([true; 4]);
        form.set_from(&matches);
        let checked = form.check(&matches).map_err(|e| e.to_string());
        (form, checked)
    }

    #[rstest]
    #[case::all_allowed([true; 4], [true; 4])]
    #[case::rules_out_what_needs_it_through_what_it_rules_out(
        [false, true, true, true],
        [false, false, false, true],
    )]
    #[case::rules_out_only_what_needs_it([true, false, true, true], [true, false, false, true])]
    fn effective_rules_out_what_the_rest_does_not_allow(
        #[case] chosen: [bool; 4],
        #[case] expected: [bool; 4],
    ) {
        assert_eq!(test(chosen).effective(), test(expected));
    }

    #[rstest]
    fn ask_says_what_each_field_needs_and_skips_what_it_does_not_ask() {
        let form = test([false, true, true, true]);
        let asked = [form.ask::<A>(), form.ask::<B>(), form.ask::<C>(), form.ask::<Unasked>()];
        assert_eq!(asked, [Some(Ok(())), Some(Err(NEEDS_A)), Some(Err(NEEDS_B)), None]);
    }

    #[rstest]
    #[case::none(&[], [true; 4])]
    #[case::off(&["--no-b"], [true, false, true, true])]
    #[case::last_wins(&["--no-b", "--b", "--no-b"], [true, false, true, true])]
    #[case::again(&["--b", "--b"], [true; 4])]
    fn flags_set_what_they_name(#[case] args: &[&str], #[case] expected: [bool; 4]) {
        assert_eq!(flagged(args), (test(expected), Ok(())));
    }

    #[rstest]
    #[case::needs_what_another_turns_off(&["--no-a", "--b"], "`--b` needs a: `--a`")]
    #[case::needs_what_another_rules_out(&["--no-a", "--c"], "`--c` needs b: `--b`")]
    #[case::not_asked(&["--unasked"], "`--unasked` can't be on here: never")]
    fn flags_that_cannot_turn_on_fail(#[case] args: &[&str], #[case] expected: &str) {
        assert_eq!(flagged(args).1, Err(expected.to_owned()));
    }

    #[rstest]
    fn flags_that_turn_off_never_fail() {
        assert_eq!(flagged(&["--no-a", "--no-unasked"]).1, Ok(()));
    }
}
