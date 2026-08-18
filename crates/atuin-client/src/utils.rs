pub(crate) fn get_hostname() -> String {
    std::env::var("ATUIN_HOST_NAME")
        .unwrap_or_else(|_| whoami::hostname().unwrap_or_else(|_| "unknown-host".to_string()))
}

pub(crate) fn get_username() -> String {
    std::env::var("ATUIN_HOST_USER")
        .unwrap_or_else(|_| whoami::username().unwrap_or_else(|_| "unknown-user".to_string()))
}
