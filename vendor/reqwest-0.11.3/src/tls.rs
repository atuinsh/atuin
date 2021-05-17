#[cfg(feature = "__rustls")]
use rustls::{RootCertStore, ServerCertVerified, ServerCertVerifier, TLSError};
use std::fmt;
#[cfg(feature = "__rustls")]
use tokio_rustls::webpki::DNSNameRef;

/// Represents a server X509 certificate.
#[derive(Clone)]
pub struct Certificate {
    #[cfg(feature = "native-tls-crate")]
    native: native_tls_crate::Certificate,
    #[cfg(feature = "__rustls")]
    original: Cert,
}

#[cfg(feature = "__rustls")]
#[derive(Clone)]
enum Cert {
    Der(Vec<u8>),
    Pem(Vec<u8>),
}

/// Represents a private key and X509 cert as a client certificate.
pub struct Identity {
    #[cfg_attr(not(any(feature = "native-tls", feature = "__rustls")), allow(unused))]
    inner: ClientCert,
}

enum ClientCert {
    #[cfg(feature = "native-tls")]
    Pkcs12(native_tls_crate::Identity),
    #[cfg(feature = "__rustls")]
    Pem {
        key: rustls::PrivateKey,
        certs: Vec<rustls::Certificate>,
    },
}

impl Certificate {
    /// Create a `Certificate` from a binary DER encoded certificate
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::fs::File;
    /// # use std::io::Read;
    /// # fn cert() -> Result<(), Box<std::error::Error>> {
    /// let mut buf = Vec::new();
    /// File::open("my_cert.der")?
    ///     .read_to_end(&mut buf)?;
    /// let cert = reqwest::Certificate::from_der(&buf)?;
    /// # drop(cert);
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_der(der: &[u8]) -> crate::Result<Certificate> {
        Ok(Certificate {
            #[cfg(feature = "native-tls-crate")]
            native: native_tls_crate::Certificate::from_der(der).map_err(crate::error::builder)?,
            #[cfg(feature = "__rustls")]
            original: Cert::Der(der.to_owned()),
        })
    }

    /// Create a `Certificate` from a PEM encoded certificate
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::fs::File;
    /// # use std::io::Read;
    /// # fn cert() -> Result<(), Box<std::error::Error>> {
    /// let mut buf = Vec::new();
    /// File::open("my_cert.pem")?
    ///     .read_to_end(&mut buf)?;
    /// let cert = reqwest::Certificate::from_pem(&buf)?;
    /// # drop(cert);
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_pem(pem: &[u8]) -> crate::Result<Certificate> {
        Ok(Certificate {
            #[cfg(feature = "native-tls-crate")]
            native: native_tls_crate::Certificate::from_pem(pem).map_err(crate::error::builder)?,
            #[cfg(feature = "__rustls")]
            original: Cert::Pem(pem.to_owned()),
        })
    }

    #[cfg(feature = "native-tls-crate")]
    pub(crate) fn add_to_native_tls(self, tls: &mut native_tls_crate::TlsConnectorBuilder) {
        tls.add_root_certificate(self.native);
    }

    #[cfg(feature = "__rustls")]
    pub(crate) fn add_to_rustls(self, tls: &mut rustls::ClientConfig) -> crate::Result<()> {
        use rustls::internal::pemfile;
        use std::io::Cursor;

        match self.original {
            Cert::Der(buf) => tls
                .root_store
                .add(&::rustls::Certificate(buf))
                .map_err(|e| crate::error::builder(TLSError::WebPKIError(e)))?,
            Cert::Pem(buf) => {
                let mut pem = Cursor::new(buf);
                let certs = pemfile::certs(&mut pem).map_err(|_| {
                    crate::error::builder(TLSError::General(String::from(
                        "No valid certificate was found",
                    )))
                })?;
                for c in certs {
                    tls.root_store
                        .add(&c)
                        .map_err(|e| crate::error::builder(TLSError::WebPKIError(e)))?;
                }
            }
        }
        Ok(())
    }
}

impl Identity {
    /// Parses a DER-formatted PKCS #12 archive, using the specified password to decrypt the key.
    ///
    /// The archive should contain a leaf certificate and its private key, as well any intermediate
    /// certificates that allow clients to build a chain to a trusted root.
    /// The chain certificates should be in order from the leaf certificate towards the root.
    ///
    /// PKCS #12 archives typically have the file extension `.p12` or `.pfx`, and can be created
    /// with the OpenSSL `pkcs12` tool:
    ///
    /// ```bash
    /// openssl pkcs12 -export -out identity.pfx -inkey key.pem -in cert.pem -certfile chain_certs.pem
    /// ```
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::fs::File;
    /// # use std::io::Read;
    /// # fn pkcs12() -> Result<(), Box<std::error::Error>> {
    /// let mut buf = Vec::new();
    /// File::open("my-ident.pfx")?
    ///     .read_to_end(&mut buf)?;
    /// let pkcs12 = reqwest::Identity::from_pkcs12_der(&buf, "my-privkey-password")?;
    /// # drop(pkcs12);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Optional
    ///
    /// This requires the `native-tls` Cargo feature enabled.
    #[cfg(feature = "native-tls")]
    pub fn from_pkcs12_der(der: &[u8], password: &str) -> crate::Result<Identity> {
        Ok(Identity {
            inner: ClientCert::Pkcs12(
                native_tls_crate::Identity::from_pkcs12(der, password)
                    .map_err(crate::error::builder)?,
            ),
        })
    }

    /// Parses PEM encoded private key and certificate.
    ///
    /// The input should contain a PEM encoded private key
    /// and at least one PEM encoded certificate.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::fs::File;
    /// # use std::io::Read;
    /// # fn pem() -> Result<(), Box<std::error::Error>> {
    /// let mut buf = Vec::new();
    /// File::open("my-ident.pem")?
    ///     .read_to_end(&mut buf)?;
    /// let id = reqwest::Identity::from_pem(&buf)?;
    /// # drop(id);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Optional
    ///
    /// This requires the `rustls-tls(-...)` Cargo feature enabled.
    #[cfg(feature = "__rustls")]
    pub fn from_pem(buf: &[u8]) -> crate::Result<Identity> {
        use rustls::internal::pemfile;
        use std::io::Cursor;

        let (key, certs) = {
            let mut pem = Cursor::new(buf);
            let certs = pemfile::certs(&mut pem)
                .map_err(|_| TLSError::General(String::from("No valid certificate was found")))
                .map_err(crate::error::builder)?;
            pem.set_position(0);
            let mut sk = pemfile::pkcs8_private_keys(&mut pem)
                .and_then(|pkcs8_keys| {
                    if pkcs8_keys.is_empty() {
                        Err(())
                    } else {
                        Ok(pkcs8_keys)
                    }
                })
                .or_else(|_| {
                    pem.set_position(0);
                    pemfile::rsa_private_keys(&mut pem)
                })
                .map_err(|_| TLSError::General(String::from("No valid private key was found")))
                .map_err(crate::error::builder)?;
            if let (Some(sk), false) = (sk.pop(), certs.is_empty()) {
                (sk, certs)
            } else {
                return Err(crate::error::builder(TLSError::General(String::from(
                    "private key or certificate not found",
                ))));
            }
        };

        Ok(Identity {
            inner: ClientCert::Pem { key, certs },
        })
    }

    #[cfg(feature = "native-tls")]
    pub(crate) fn add_to_native_tls(
        self,
        tls: &mut native_tls_crate::TlsConnectorBuilder,
    ) -> crate::Result<()> {
        match self.inner {
            ClientCert::Pkcs12(id) => {
                tls.identity(id);
                Ok(())
            }
            #[cfg(feature = "__rustls")]
            ClientCert::Pem { .. } => Err(crate::error::builder("incompatible TLS identity type")),
        }
    }

    #[cfg(feature = "__rustls")]
    pub(crate) fn add_to_rustls(self, tls: &mut rustls::ClientConfig) -> crate::Result<()> {
        match self.inner {
            ClientCert::Pem { key, certs } => {
                tls.set_single_client_cert(certs, key)
                    .map_err(|e| crate::error::builder(e))?;
                Ok(())
            }
            #[cfg(feature = "native-tls")]
            ClientCert::Pkcs12(..) => Err(crate::error::builder("incompatible TLS identity type")),
        }
    }
}

impl fmt::Debug for Certificate {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Certificate").finish()
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Identity").finish()
    }
}

pub(crate) enum TlsBackend {
    #[cfg(feature = "default-tls")]
    Default,
    #[cfg(feature = "native-tls")]
    BuiltNativeTls(native_tls_crate::TlsConnector),
    #[cfg(feature = "__rustls")]
    Rustls,
    #[cfg(feature = "__rustls")]
    BuiltRustls(rustls::ClientConfig),
    #[cfg(any(feature = "native-tls", feature = "__rustls",))]
    UnknownPreconfigured,
}

impl fmt::Debug for TlsBackend {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            #[cfg(feature = "default-tls")]
            TlsBackend::Default => write!(f, "Default"),
            #[cfg(feature = "native-tls")]
            TlsBackend::BuiltNativeTls(_) => write!(f, "BuiltNativeTls"),
            #[cfg(feature = "__rustls")]
            TlsBackend::Rustls => write!(f, "Rustls"),
            #[cfg(feature = "__rustls")]
            TlsBackend::BuiltRustls(_) => write!(f, "BuiltRustls"),
            #[cfg(any(feature = "native-tls", feature = "__rustls",))]
            TlsBackend::UnknownPreconfigured => write!(f, "UnknownPreconfigured"),
        }
    }
}

impl Default for TlsBackend {
    fn default() -> TlsBackend {
        #[cfg(feature = "default-tls")]
        {
            TlsBackend::Default
        }

        #[cfg(all(feature = "__rustls", not(feature = "default-tls")))]
        {
            TlsBackend::Rustls
        }
    }
}

#[cfg(feature = "__rustls")]
pub(crate) struct NoVerifier;

#[cfg(feature = "__rustls")]
impl ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _roots: &RootCertStore,
        _presented_certs: &[rustls::Certificate],
        _dns_name: DNSNameRef,
        _ocsp_response: &[u8],
    ) -> Result<ServerCertVerified, TLSError> {
        Ok(ServerCertVerified::assertion())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "default-tls")]
    #[test]
    fn certificate_from_der_invalid() {
        Certificate::from_der(b"not der").unwrap_err();
    }

    #[cfg(feature = "default-tls")]
    #[test]
    fn certificate_from_pem_invalid() {
        Certificate::from_pem(b"not pem").unwrap_err();
    }

    #[cfg(feature = "native-tls")]
    #[test]
    fn identity_from_pkcs12_der_invalid() {
        Identity::from_pkcs12_der(b"not der", "nope").unwrap_err();
    }

    #[cfg(feature = "__rustls")]
    #[test]
    fn identity_from_pem_invalid() {
        Identity::from_pem(b"not pem").unwrap_err();
    }

    #[cfg(feature = "__rustls")]
    #[test]
    fn identity_from_pem_pkcs1_key() {
        let pem = b"-----BEGIN CERTIFICATE-----\n\
            -----END CERTIFICATE-----\n\
            -----BEGIN RSA PRIVATE KEY-----\n\
            -----END RSA PRIVATE KEY-----\n";

        Identity::from_pem(pem).unwrap();
    }
}
