use std::ffi::OsStr;

pub(crate) fn get_hostname() -> String {
    std::env::var("ATUIN_HOST_NAME").unwrap_or_else(|_| {
        whoami::fallible::hostname().unwrap_or_else(|_| "unknown-host".to_string())
    })
}

pub(crate) fn get_username() -> String {
    std::env::var("ATUIN_HOST_USER").unwrap_or_else(|_| whoami::username())
}

/// Returns a pair of the hostname and username, separated by a colon.
pub(crate) fn get_host_user() -> String {
    format!("{}:{}", get_hostname(), get_username())
}

pub(crate) fn get_env_var<K: AsRef<OsStr>>(key: K) -> Option<String> {
    // Try to retrieve the environment variable using std::env::var.
    // If it fails (e.g., variable contains non-UTF-8 bytes), fall back to std::env::var_os and convert it lossily.
    std::env::var(key.as_ref())
        .ok()
        .or_else(|| std::env::var_os(key.as_ref()).map(|v| v.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;
    use std::{env, ffi};

    #[test]
    fn test_get_env_var_existing() {
        // Set an environment variable for the purpose of this test.
        env::set_var("TEST_ENV_VAR", "123");
        assert_eq!(get_env_var("TEST_ENV_VAR"), Some("123".to_string()));
        env::remove_var("TEST_ENV_VAR");
    }

    #[test]
    fn test_get_env_var_non_existing() {
        // Ensure the environment variable does not exist.
        env::remove_var("NON_EXISTING_ENV_VAR");
        assert_eq!(get_env_var("NON_EXISTING_ENV_VAR"), None);
    }

    #[test]
    fn test_get_env_var_non_utf8() {
        // Create a non-UTF-8 sequence.
        let invalid_utf8: &[u8] = &[0xff, 0xfe, 0xfd];
        let os_str = ffi::OsStr::from_bytes(invalid_utf8);

        // Set an environment variable with non-UTF-8 bytes for the purpose of this test.
        env::set_var("TEST_NON_UTF8", os_str);
        assert_eq!(
            get_env_var("TEST_NON_UTF8").unwrap(),
            "\u{FFFD}\u{FFFD}\u{FFFD}"
        );
        env::remove_var("TEST_NON_UTF8");
    }
}
