use std::borrow::Cow;
use std::env::var_os;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

/// try to load a password from the various pgpass file locations
pub fn load_password(
    host: &str,
    port: u16,
    username: &str,
    database: Option<&str>,
) -> Option<String> {
    let custom_file = var_os("PGPASSFILE");
    if let Some(file) = custom_file {
        if let Some(password) =
            load_password_from_file(PathBuf::from(file), host, port, username, database)
        {
            return Some(password);
        }
    }

    #[cfg(not(target_os = "windows"))]
    let default_file = dirs::home_dir().map(|path| path.join(".pgpass"));
    #[cfg(target_os = "windows")]
    let default_file = dirs::data_dir().map(|path| path.join("postgres").join("pgpass.conf"));
    load_password_from_file(default_file?, host, port, username, database)
}

/// try to extract a password from a pgpass file
fn load_password_from_file(
    path: PathBuf,
    host: &str,
    port: u16,
    username: &str,
    database: Option<&str>,
) -> Option<String> {
    let file = File::open(&path).ok()?;

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;

        // check file permissions on linux

        let metadata = file.metadata().ok()?;
        let permissions = metadata.permissions();
        let mode = permissions.mode();
        if mode & 0o77 != 0 {
            log::warn!(
                "ignoring {}: permissions for not strict enough: {:o}",
                path.to_string_lossy(),
                mode
            );
            return None;
        }
    }

    let mut reader = BufReader::new(file);
    let mut line = String::new();

    while let Ok(n) = reader.read_line(&mut line) {
        if n == 0 {
            break;
        }

        if line.starts_with('#') {
            // comment, do nothing
        } else {
            // try to load password from line
            let line = &line[..line.len() - 1]; // trim newline
            if let Some(password) = load_password_from_line(line, host, port, username, database) {
                return Some(password);
            }
        }

        line.clear();
    }

    None
}

/// try to check all fields & extract the password
fn load_password_from_line(
    mut line: &str,
    host: &str,
    port: u16,
    username: &str,
    database: Option<&str>,
) -> Option<String> {
    let whole_line = line;
    matches_next_field(whole_line, &mut line, host)?;
    matches_next_field(whole_line, &mut line, &port.to_string())?;
    matches_next_field(whole_line, &mut line, username)?;
    matches_next_field(whole_line, &mut line, database.unwrap_or_default())?;
    Some(line.to_owned())
}

/// check if the next field matches the provided value
fn matches_next_field(whole_line: &str, line: &mut &str, value: &str) -> Option<()> {
    let field = find_next_field(line);
    match field {
        Some(field) => {
            if field == "*" || field == value {
                Some(())
            } else {
                None
            }
        }
        None => {
            log::warn!("Malformed line in pgpass file: {}", whole_line);
            None
        }
    }
}

/// extract the next value from a line in a pgpass file
///
/// `line` will get updated to point behind the field and delimiter
fn find_next_field<'a>(line: &mut &'a str) -> Option<Cow<'a, str>> {
    let mut escaping = false;
    let mut escaped_string = None;
    let mut last_added = 0;

    let char_indicies = line.char_indices();
    for (idx, c) in char_indicies {
        if c == ':' && !escaping {
            let (field, rest) = line.split_at(idx);
            *line = &rest[1..];

            if let Some(mut escaped_string) = escaped_string {
                escaped_string += &field[last_added..];
                return Some(Cow::Owned(escaped_string));
            } else {
                return Some(Cow::Borrowed(field));
            }
        } else if c == '\\' {
            let s = escaped_string.get_or_insert_with(String::new);

            if escaping {
                s.push('\\');
            } else {
                *s += &line[last_added..idx];
            }

            escaping = !escaping;
            last_added = idx + 1;
        } else {
            escaping = false;
        }
    }

    return None;
}

#[cfg(test)]
mod tests {
    use super::{find_next_field, load_password_from_line};
    use std::borrow::Cow;

    #[test]
    fn test_find_next_field() {
        fn test_case<'a>(mut input: &'a str, result: Option<Cow<'a, str>>, rest: &str) {
            assert_eq!(find_next_field(&mut input), result);
            assert_eq!(input, rest);
        }

        // normal field
        test_case("foo:bar:baz", Some(Cow::Borrowed("foo")), "bar:baz");
        // \ escaped
        test_case(
            "foo\\\\:bar:baz",
            Some(Cow::Owned("foo\\".to_owned())),
            "bar:baz",
        );
        // : escaped
        test_case(
            "foo\\::bar:baz",
            Some(Cow::Owned("foo:".to_owned())),
            "bar:baz",
        );
        // unnecessary escape
        test_case(
            "foo\\a:bar:baz",
            Some(Cow::Owned("fooa".to_owned())),
            "bar:baz",
        );
        // other text after escape
        test_case(
            "foo\\\\a:bar:baz",
            Some(Cow::Owned("foo\\a".to_owned())),
            "bar:baz",
        );
        // double escape
        test_case(
            "foo\\\\\\\\a:bar:baz",
            Some(Cow::Owned("foo\\\\a".to_owned())),
            "bar:baz",
        );
        // utf8 support
        test_case("🦀:bar:baz", Some(Cow::Borrowed("🦀")), "bar:baz");

        // missing delimiter (eof)
        test_case("foo", None, "foo");
        // missing delimiter after escape
        test_case("foo\\:", None, "foo\\:");
        // missing delimiter after unused trailing escape
        test_case("foo\\", None, "foo\\");
    }

    #[test]
    fn test_load_password_from_line() {
        // normal
        assert_eq!(
            load_password_from_line(
                "localhost:5432:foo:bar:baz",
                "localhost",
                5432,
                "foo",
                Some("bar")
            ),
            Some("baz".to_owned())
        );
        // wildcard
        assert_eq!(
            load_password_from_line("*:5432:foo:bar:baz", "localhost", 5432, "foo", Some("bar")),
            Some("baz".to_owned())
        );
        // accept wildcard with missing db
        assert_eq!(
            load_password_from_line("localhost:5432:foo:*:baz", "localhost", 5432, "foo", None),
            Some("baz".to_owned())
        );

        // doesn't match
        assert_eq!(
            load_password_from_line(
                "thishost:5432:foo:bar:baz",
                "thathost",
                5432,
                "foo",
                Some("bar")
            ),
            None
        );
        // malformed entry
        assert_eq!(
            load_password_from_line(
                "localhost:5432:foo:bar",
                "localhost",
                5432,
                "foo",
                Some("bar")
            ),
            None
        );
    }
}
