// https://github.com/TeXitoi/structopt/issues/{NUMBER}

mod utils;
use utils::*;

use structopt::StructOpt;

#[test]
fn issue_151() {
    use structopt::{clap::ArgGroup, StructOpt};

    #[derive(StructOpt, Debug)]
    #[structopt(group = ArgGroup::with_name("verb").required(true).multiple(true))]
    struct Opt {
        #[structopt(long, group = "verb")]
        foo: bool,
        #[structopt(long, group = "verb")]
        bar: bool,
    }

    #[derive(Debug, StructOpt)]
    struct Cli {
        #[structopt(flatten)]
        a: Opt,
    }

    assert!(Cli::clap().get_matches_from_safe(&["test"]).is_err());
    assert!(Cli::clap()
        .get_matches_from_safe(&["test", "--foo"])
        .is_ok());
    assert!(Cli::clap()
        .get_matches_from_safe(&["test", "--bar"])
        .is_ok());
    assert!(Cli::clap()
        .get_matches_from_safe(&["test", "--zebra"])
        .is_err());
    assert!(Cli::clap()
        .get_matches_from_safe(&["test", "--foo", "--bar"])
        .is_ok());
}

#[test]
fn issue_289() {
    use structopt::{clap::AppSettings, StructOpt};

    #[derive(StructOpt)]
    #[structopt(setting = AppSettings::InferSubcommands)]
    enum Args {
        SomeCommand(SubSubCommand),
        AnotherCommand,
    }

    #[derive(StructOpt)]
    #[structopt(setting = AppSettings::InferSubcommands)]
    enum SubSubCommand {
        TestCommand,
    }

    assert!(Args::clap()
        .get_matches_from_safe(&["test", "some-command", "test-command"])
        .is_ok());
    assert!(Args::clap()
        .get_matches_from_safe(&["test", "some", "test-command"])
        .is_ok());
    assert!(Args::clap()
        .get_matches_from_safe(&["test", "some-command", "test"])
        .is_ok());
    assert!(Args::clap()
        .get_matches_from_safe(&["test", "some", "test"])
        .is_ok());
}

#[test]
fn issue_324() {
    fn my_version() -> &'static str {
        "MY_VERSION"
    }

    #[derive(StructOpt)]
    #[structopt(version = my_version())]
    struct Opt {
        #[structopt(subcommand)]
        _cmd: Option<SubCommand>,
    }

    #[derive(StructOpt)]
    enum SubCommand {
        Start,
    }

    let help = get_long_help::<Opt>();
    assert!(help.contains("MY_VERSION"));
}

#[test]
fn issue_359() {
    #[derive(Debug, PartialEq, StructOpt)]
    struct Opt {
        #[structopt(subcommand)]
        sub: Subcommands,
    }

    #[derive(Debug, PartialEq, StructOpt)]
    enum Subcommands {
        Add,

        #[structopt(external_subcommand)]
        Other(Vec<String>),
    }

    assert_eq!(
        Opt {
            sub: Subcommands::Other(vec!["only_one_arg".into()])
        },
        Opt::from_iter(&["test", "only_one_arg"])
    );
}
