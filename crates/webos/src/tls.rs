//! Checking the TV's TLS certificate.
//!
//! The TV's certificate names no host a client could check, and it is the same on every LG TV:
//! `CN=LGE TV SSG`, issued by `LGE SSG Intermediate CA`. Pinning it therefore proves that an LG
//! TV answers, not which one; the `deviceUUID` in the TV's `hello` does that. Handshake signatures
//! are verified as usual, so the peer must hold the certificate's key.

use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, OtherError, SignatureScheme};
use sha2::{Digest, Sha256};

/// SHA-256 fingerprint of a certificate in DER form. Shown and parsed as 32 hex byte pairs
/// separated by colons, like `openssl x509 -fingerprint -sha256` prints it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub fn of(der: &[u8]) -> Self {
        Self(Sha256::digest(der).into())
    }
}

/// The certificate LG TVs present on the TLS port, valid 2018-03-12 to 2034-08-15. Published by
/// the lgtv2 library, which checks it on a webOS 6.0 TV with firmware 03.53.45.
pub const LG_TV_CERTIFICATE: Fingerprint = Fingerprint([
    0x11, 0xC5, 0xB1, 0xC5, 0x90, 0x77, 0x50, 0xAB, 0xB9, 0xDA, 0x2A, 0x66, 0x65, 0xCC, 0xCE, 0x2B,
    0xB2, 0x88, 0xA5, 0x83, 0xF4, 0x5A, 0x33, 0x39, 0xE7, 0x1F, 0x87, 0xBF, 0x2F, 0x80, 0x85, 0x52,
]);

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

/// The text is not a SHA-256 fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a SHA-256 fingerprint: expected 64 hex digits, optionally separated by colons")]
pub struct ParseFingerprintError;

impl FromStr for Fingerprint {
    type Err = ParseFingerprintError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let digits: Vec<u8> = text.bytes().filter(|&b| b != b':').collect();
        if digits.len() != 64 || !digits.iter().all(u8::is_ascii_hexdigit) {
            return Err(ParseFingerprintError);
        }
        let mut bytes = [0; 32];
        for (byte, pair) in bytes.iter_mut().zip(digits.as_chunks::<2>().0) {
            let pair = std::str::from_utf8(pair).map_err(|_| ParseFingerprintError)?;
            *byte = u8::from_str_radix(pair, 16).map_err(|_| ParseFingerprintError)?;
        }
        Ok(Self(bytes))
    }
}

/// The TV presented a certificate outside the accepted ones.
#[derive(Debug, thiserror::Error)]
#[error("the TV presented certificate {0}, which is not an LG TV certificate")]
struct Unknown(Fingerprint);

/// Accepts the certificates in `accepted` and records the one it saw.
#[derive(Debug)]
struct PinVerifier {
    accepted: Vec<Fingerprint>,
    seen: Arc<Mutex<Option<Fingerprint>>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let found = Fingerprint::of(end_entity);
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(found);
        if self.accepted.contains(&found) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(CertificateError::Other(
                OtherError(Arc::new(Unknown(found))),
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// TLS settings for talking to TVs, and the certificate presented in the latest handshake.
#[derive(Debug, Clone)]
pub struct Tls {
    pub(crate) config: Arc<ClientConfig>,
    seen: Arc<Mutex<Option<Fingerprint>>>,
}

impl Tls {
    /// Accepts LG's TV certificate, [`LG_TV_CERTIFICATE`].
    pub fn lg() -> Result<Self, rustls::Error> {
        Self::accepting(vec![LG_TV_CERTIFICATE])
    }

    /// Accepts only certificates with these fingerprints. TLS 1.2 stays on: the TVs speak it.
    pub fn accepting(accepted: Vec<Fingerprint>) -> Result<Self, rustls::Error> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let seen = Arc::default();
        let verifier = Arc::new(PinVerifier {
            accepted,
            seen: Arc::clone(&seen),
            provider: Arc::clone(&provider),
        });
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        Ok(Self {
            config: Arc::new(config),
            seen,
        })
    }

    /// Fingerprint of the certificate in the latest handshake.
    pub fn seen(&self) -> Option<Fingerprint> {
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_print_and_parse_like_openssl() {
        let print = Fingerprint::of(b"certificate");
        let text = print.to_string();
        assert_eq!(text.len(), 32 * 3 - 1);
        assert_eq!(text.parse::<Fingerprint>(), Ok(print));
        assert_eq!(text.replace(':', "").to_lowercase().parse(), Ok(print));
        assert_eq!("AB:CD".parse::<Fingerprint>(), Err(ParseFingerprintError));
        assert_eq!(
            "zz".repeat(32).parse::<Fingerprint>(),
            Err(ParseFingerprintError)
        );
    }

    #[test]
    fn the_lg_certificate_reads_as_published() {
        assert_eq!(
            LG_TV_CERTIFICATE.to_string(),
            "11:C5:B1:C5:90:77:50:AB:B9:DA:2A:66:65:CC:CE:2B:B2:88:A5:83:F4:5A:33:39:E7:1F:87:BF:2F:80:85:52"
        );
    }
}
