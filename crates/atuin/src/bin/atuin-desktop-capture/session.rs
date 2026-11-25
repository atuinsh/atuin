//! What the helper's commands share about the desktop session: waking its applications, and
//! the variables that decide whether applications expose their content.

use atuin_common::string::wrap;
use atuin_desktop_capture::{Backend, Desktop, Exposure, Platform as _, SessionVariable};

/// How wide the helper's paragraphs are, including their indent.
const COLUMNS: usize = 90;

/// Whether waking the running applications asked any of them to expose its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Woken {
    Asked,
    AllReady,
}

/// Asks the running applications of `backend` to expose their content, and waits as long as
/// they need. Prints `notice` before the wait.
pub async fn wake_apps<B: Backend>(backend: &B, notice: &str) -> Woken {
    match backend.wake_apps().await {
        Some(wait) => {
            println!("{notice}");
            tokio::time::sleep(wait).await;
            Woken::Asked
        }
        None => Woken::AllReady,
    }
}

/// Prints `text` as a paragraph whose lines start with `indent`.
pub fn print_paragraph(indent: &str, text: &str) {
    for line in wrap(text, COLUMNS.saturating_sub(indent.len())) {
        println!("{indent}{line}");
    }
}

/// Tells, for each variable of the session, whether the variable keeps applications from
/// exposing their content. Each line starts with `indent`.
///
/// `atuin setup` writes the variables into the files that sessions read at login, so a session
/// has them only after a login that follows `atuin setup`.
pub fn print_session_variables(indent: &str) {
    let variables = Desktop::session_variables();
    if variables.is_empty() {
        println!("{indent}Not needed on this platform.");
    }
    for variable in variables {
        if variable.hides_in_this_session() {
            print_hiding(indent, variable);
        } else if let Exposure::Requires(value) = variable.exposure {
            println!("{indent}{}={value} is active in this session.", variable.name);
        }
    }
}

/// Prints the variables of this session that keep applications from exposing their content.
/// Each line starts with `indent`, and the first line is blank.
pub fn print_hiding_variables(indent: &str) {
    for variable in Desktop::session_variables() {
        if variable.hides_in_this_session() {
            println!();
            print_hiding(indent, variable);
        }
    }
}

/// Prints what `variable` does and how to fix it, for a `variable` that keeps applications
/// from exposing their content in this session.
fn print_hiding(indent: &str, variable: &SessionVariable) {
    let SessionVariable {
        name, hiding_value, ..
    } = variable;
    print_paragraph(indent, variable.reason);
    let fix = match variable.exposure {
        Exposure::Requires(value) => format!(
            "This session runs without {name}={value}. `atuin setup` sets it in the files that \
             sessions read at login, so log out and in after `atuin setup`."
        ),
        Exposure::Overrides(value) => format!(
            "This session sets {name}={hiding_value}. `atuin setup` sets {name}={value} in the \
             files that sessions read at login, so log out and in after `atuin setup`."
        ),
        Exposure::Unless => format!(
            "This session sets {name}={hiding_value}. Remove the assignment from the file that \
             sets it, and log out and in."
        ),
    };
    print_paragraph(indent, &fix);
}
