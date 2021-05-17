use webpki;

use crate::key;
#[cfg(feature = "logging")]
use crate::log::{debug, trace};
pub use crate::msgs::handshake::{DistinguishedName, DistinguishedNames};
use crate::pemfile;
use crate::x509;
use std::io;

/// This is like a `webpki::TrustAnchor`, except it owns
/// rather than borrows its memory.  That prevents lifetimes
/// leaking up the object tree.
#[derive(Debug, Clone)]
pub struct OwnedTrustAnchor {
    subject: Vec<u8>,
    spki: Vec<u8>,
    name_constraints: Option<Vec<u8>>,
}

impl OwnedTrustAnchor {
    /// Copy a `webpki::TrustAnchor` into owned memory
    pub fn from_trust_anchor(t: &webpki::TrustAnchor) -> OwnedTrustAnchor {
        OwnedTrustAnchor {
            subject: t.subject.to_vec(),
            spki: t.spki.to_vec(),
            name_constraints: t.name_constraints.map(|x| x.to_vec()),
        }
    }

    /// Get a `webpki::TrustAnchor` by borrowing the owned elements.
    pub fn to_trust_anchor(&self) -> webpki::TrustAnchor {
        webpki::TrustAnchor {
            subject: &self.subject,
            spki: &self.spki,
            name_constraints: self
                .name_constraints
                .as_ref()
                .map(Vec::as_slice),
        }
    }
}

impl From<webpki::TrustAnchor<'_>> for OwnedTrustAnchor {
    fn from(t: webpki::TrustAnchor) -> OwnedTrustAnchor {
        Self::from_trust_anchor(&t)
    }
}

impl<'a> Into<webpki::TrustAnchor<'a>> for &'a OwnedTrustAnchor {
    fn into(self) -> webpki::TrustAnchor<'a> {
        self.to_trust_anchor()
    }
}

/// A container for root certificates able to provide a root-of-trust
/// for connection authentication.
#[derive(Debug, Clone)]
pub struct RootCertStore {
    /// The list of roots.
    pub roots: Vec<OwnedTrustAnchor>,
}

impl RootCertStore {
    /// Make a new, empty `RootCertStore`.
    pub fn empty() -> RootCertStore {
        RootCertStore { roots: Vec::new() }
    }

    /// Return true if there are no certificates.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Say how many certificates are in the container.
    pub fn len(&self) -> usize {
        self.roots.len()
    }

    /// Return the Subject Names for certificates in the container.
    pub fn get_subjects(&self) -> DistinguishedNames {
        let mut r = DistinguishedNames::new();

        for ota in &self.roots {
            let mut name = Vec::new();
            name.extend_from_slice(&ota.subject);
            x509::wrap_in_sequence(&mut name);
            r.push(DistinguishedName::new(name));
        }

        r
    }

    /// Add a single DER-encoded certificate to the store.
    pub fn add(&mut self, der: &key::Certificate) -> Result<(), webpki::Error> {
        let ta = webpki::trust_anchor_util::cert_der_as_trust_anchor(&der.0)?;

        let ota = OwnedTrustAnchor::from_trust_anchor(&ta);
        self.roots.push(ota);
        Ok(())
    }

    /// Adds all the given TrustAnchors `anchors`.  This does not
    /// fail.
    pub fn add_server_trust_anchors(
        &mut self,
        &webpki::TLSServerTrustAnchors(anchors): &webpki::TLSServerTrustAnchors,
    ) {
        for ta in anchors {
            self.roots
                .push(OwnedTrustAnchor::from_trust_anchor(ta));
        }
    }

    /// Parse a PEM file and add all certificates found inside.
    /// Errors are non-specific; they may be io errors in `rd` and
    /// PEM format errors, but not certificate validity errors.
    ///
    /// This is because large collections of root certificates often
    /// include ancient or syntactically invalid certificates.  CAs
    /// are competent like that.
    ///
    /// Returns the number of certificates added, and the number
    /// which were extracted from the PEM but ultimately unsuitable.
    pub fn add_pem_file(&mut self, rd: &mut dyn io::BufRead) -> Result<(usize, usize), ()> {
        let ders = pemfile::certs(rd)?;
        let mut valid_count = 0;
        let mut invalid_count = 0;

        for der in ders {
            #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
            match self.add(&der) {
                Ok(_) => valid_count += 1,
                Err(err) => {
                    trace!("invalid cert der {:?}", der);
                    debug!("certificate parsing failed: {:?}", err);
                    invalid_count += 1
                }
            }
        }

        debug!(
            "add_pem_file processed {} valid and {} invalid certs",
            valid_count, invalid_count
        );

        Ok((valid_count, invalid_count))
    }
}
