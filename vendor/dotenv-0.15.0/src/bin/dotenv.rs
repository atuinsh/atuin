extern crate clap;
extern crate dotenv;

use clap::{App, AppSettings, Arg};
use std::os::unix::process::CommandExt;
use std::process::{Command, exit};

macro_rules! die {
    ($fmt:expr) => ({
        eprintln!($fmt);
        exit(1);
    });
    ($fmt:expr, $($arg:tt)*) => ({
        eprintln!($fmt, $($arg)*);
        exit(1);
    });
}

fn make_command(name: &str, args: Vec<&str>) -> Command {
    let mut command = Command::new(name);

    for arg in args {
        command.arg(arg);
    }

    return command;
}

fn main() {
    let matches = App::new("dotenv")
        .about("Run a command using the environment in a .env file")
        .usage("dotenv <COMMAND> [ARGS]...")
        .setting(AppSettings::AllowExternalSubcommands)
        .setting(AppSettings::ArgRequiredElseHelp)
        .setting(AppSettings::UnifiedHelpMessage)
        .arg(Arg::with_name("FILE")
             .short("f")
             .long("file")
             .takes_value(true)
             .help("Use a specific .env file (defaults to .env)"))
        .get_matches();

    match matches.value_of("FILE") {
        None => dotenv::dotenv(),
        Some(file) => dotenv::from_filename(file),
    }.unwrap_or_else(|e| die!("error: failed to load environment: {}", e));

    let mut command = match matches.subcommand() {
        (name, Some(matches)) => {
            let args = matches.values_of("")
                .map(|v| v.collect())
                .unwrap_or(Vec::new());

            make_command(name, args)
        },
        _ => die!("error: missing required argument <COMMAND>"),
    };

    if cfg!(target_os = "windows") {
        match command.spawn().and_then(|mut child| child.wait()) {
            Ok(status) => exit(status.code().unwrap_or(1)),
            Err(error) => die!("fatal: {}", error),
        };
    } else {
        let error = command.exec();
        die!("fatal: {}", error);
    };
}
