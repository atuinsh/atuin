pub(crate) fn get_hostname() -> String {
    std::env::var("ATUIN_HOST_NAME")
        .unwrap_or_else(|_| whoami::hostname().unwrap_or_else(|_| "unknown-host".to_string()))
}

pub(crate) fn get_username() -> String {
    std::env::var("ATUIN_HOST_USER")
        .unwrap_or_else(|_| whoami::username().unwrap_or_else(|_| "unknown-user".to_string()))
}

/// Returns a pair of the hostname and username, separated by a colon.
pub(crate) fn get_host_user() -> String {
    format!("{}:{}", get_hostname(), get_username())
}
