// Copyright 2015 Brian Smith.
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

//! Utilities for efficiently embedding trust anchors in programs.

use super::der;
use crate::{
    cert::{certificate_serial_number, parse_cert_internal, Cert, EndEntityOrCA},
    Error, TrustAnchor,
};

/// Interprets the given DER-encoded certificate as a `TrustAnchor`. The
/// certificate is not validated. In particular, there is no check that the
/// certificate is self-signed or even that the certificate has the cA basic
/// constraint.
pub fn cert_der_as_trust_anchor(cert_der: &[u8]) -> Result<TrustAnchor, Error> {
    let cert_der = untrusted::Input::from(cert_der);

    // XXX: `EndEntityOrCA::EndEntity` is used instead of `EndEntityOrCA::CA`
    // because we don't have a reference to a child cert, which is needed for
    // `EndEntityOrCA::CA`. For this purpose, it doesn't matter.
    //
    // v1 certificates will result in `Error::BadDER` because `parse_cert` will
    // expect a version field that isn't there. In that case, try to parse the
    // certificate using a special parser for v1 certificates. Notably, that
    // parser doesn't allow extensions, so there's no need to worry about
    // embedded name constraints in a v1 certificate.
    match parse_cert_internal(
        cert_der,
        EndEntityOrCA::EndEntity,
        possibly_invalid_certificate_serial_number,
    ) {
        Ok(cert) => Ok(trust_anchor_from_cert(cert)),
        Err(Error::BadDER) => parse_cert_v1(cert_der).or(Err(Error::BadDER)),
        Err(err) => Err(err),
    }
}

fn possibly_invalid_certificate_serial_number<'a>(
    input: &mut untrusted::Reader<'a>,
) -> Result<(), Error> {
    // https://tools.ietf.org/html/rfc5280#section-4.1.2.2:
    // * Conforming CAs MUST NOT use serialNumber values longer than 20 octets."
    // * "The serial number MUST be a positive integer [...]"
    //
    // However, we don't enforce these constraints on trust anchors, as there
    // are widely-deployed trust anchors that violate these constraints.
    skip(input, der::Tag::Integer)
}

/// Generates code for hard-coding the given trust anchors into a program. This
/// is designed to be used in a build script. `name` is the name of the public
/// static variable that will contain the TrustAnchor array.
pub fn generate_code_for_trust_anchors(name: &str, trust_anchors: &[TrustAnchor]) -> String {
    let decl = format!(
        "static {}: [TrustAnchor<'static>; {}] = ",
        name,
        trust_anchors.len()
    );

    // "{:?}" formats the array of trust anchors as Rust code, approximately,
    // except that it drops the leading "&" on slices.
    let value = str::replace(&format!("{:?};\n", trust_anchors), ": [", ": &[");

    decl + &value
}

fn trust_anchor_from_cert<'a>(cert: Cert<'a>) -> TrustAnchor<'a> {
    TrustAnchor {
        subject: cert.subject.as_slice_less_safe(),
        spki: cert.spki.value().as_slice_less_safe(),
        name_constraints: cert.name_constraints.map(|nc| nc.as_slice_less_safe()),
    }
}

/// Parses a v1 certificate directly into a TrustAnchor.
fn parse_cert_v1<'a>(cert_der: untrusted::Input<'a>) -> Result<TrustAnchor<'a>, Error> {
    // X.509 Certificate: https://tools.ietf.org/html/rfc5280#section-4.1.
    cert_der.read_all(Error::BadDER, |cert_der| {
        der::nested(cert_der, der::Tag::Sequence, Error::BadDER, |cert_der| {
            let anchor = der::nested(cert_der, der::Tag::Sequence, Error::BadDER, |tbs| {
                // The version number field does not appear in v1 certificates.
                certificate_serial_number(tbs)?;

                skip(tbs, der::Tag::Sequence)?; // signature.
                skip(tbs, der::Tag::Sequence)?; // issuer.
                skip(tbs, der::Tag::Sequence)?; // validity.
                let subject = der::expect_tag_and_get_value(tbs, der::Tag::Sequence)?;
                let spki = der::expect_tag_and_get_value(tbs, der::Tag::Sequence)?;

                Ok(TrustAnchor {
                    subject: subject.as_slice_less_safe(),
                    spki: spki.as_slice_less_safe(),
                    name_constraints: None,
                })
            });

            // read and discard signatureAlgorithm + signature
            skip(cert_der, der::Tag::Sequence)?;
            skip(cert_der, der::Tag::BitString)?;

            anchor
        })
    })
}

fn skip(input: &mut untrusted::Reader, tag: der::Tag) -> Result<(), Error> {
    der::expect_tag_and_get_value(input, tag).map(|_| ())
}
