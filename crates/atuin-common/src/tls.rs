use std::sync::Once;

static INIT: Once = Once::new();

/// Ensure the rustls crypto provider (ring) is installed.
///
/// Must be called before creating any reqwest clients. Safe to call
/// multiple times — only the first call installs the provider.
pub fn ensure_crypto_provider() {
    INIT.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Failed to install rustls crypto provider");
    });
}
