use crate::net::CertificateInput;
use rustls::{
    Certificate, ClientConfig, RootCertStore, ServerCertVerified, ServerCertVerifier, TLSError,
    WebPKIVerifier,
};
use std::io::Cursor;
use std::sync::Arc;
use webpki::DNSNameRef;

use crate::error::Error;

pub async fn configure_tls_connector(
    accept_invalid_certs: bool,
    accept_invalid_hostnames: bool,
    root_cert_path: Option<&CertificateInput>,
) -> Result<sqlx_rt::TlsConnector, Error> {
    let mut config = ClientConfig::new();

    if accept_invalid_certs {
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(DummyTlsVerifier));
    } else {
        config
            .root_store
            .add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);

        if let Some(ca) = root_cert_path {
            let data = ca.data().await?;
            let mut cursor = Cursor::new(data);
            config
                .root_store
                .add_pem_file(&mut cursor)
                .map_err(|_| Error::Tls(format!("Invalid certificate {}", ca).into()))?;
        }

        if accept_invalid_hostnames {
            config
                .dangerous()
                .set_certificate_verifier(Arc::new(NoHostnameTlsVerifier));
        }
    }

    Ok(Arc::new(config).into())
}

struct DummyTlsVerifier;

impl ServerCertVerifier for DummyTlsVerifier {
    fn verify_server_cert(
        &self,
        _roots: &RootCertStore,
        _presented_certs: &[Certificate],
        _dns_name: DNSNameRef<'_>,
        _ocsp_response: &[u8],
    ) -> Result<ServerCertVerified, TLSError> {
        Ok(ServerCertVerified::assertion())
    }
}

pub struct NoHostnameTlsVerifier;

impl ServerCertVerifier for NoHostnameTlsVerifier {
    fn verify_server_cert(
        &self,
        roots: &RootCertStore,
        presented_certs: &[Certificate],
        dns_name: DNSNameRef<'_>,
        ocsp_response: &[u8],
    ) -> Result<ServerCertVerified, TLSError> {
        let verifier = WebPKIVerifier::new();
        match verifier.verify_server_cert(roots, presented_certs, dns_name, ocsp_response) {
            Err(TLSError::WebPKIError(webpki::Error::CertNotValidForName)) => {
                Ok(ServerCertVerified::assertion())
            }
            res => res,
        }
    }
}
