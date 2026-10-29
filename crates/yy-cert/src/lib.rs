//! Certificate signing with RustCrypto, without rcgen's native crypto backends.

use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use p256::elliptic_curve::Generate;
use p256::pkcs8::{EncodePrivateKey, LineEnding};
use rcgen::{CertificateParams, KeyIdMethod, PublicKeyData, SignatureAlgorithm};
use sha2::{Digest, Sha256};

/// An ECDSA P-256/SHA-256 key. Secret material is zeroized by RustCrypto.
pub struct KeyPair {
    key: SigningKey,
    public: p256::Sec1Point,
}

impl KeyPair {
    /// Generate a key using the operating system's cryptographic random source.
    pub fn generate() -> Result<Self, String> {
        let key = SigningKey::try_generate().map_err(|e| e.to_string())?;
        let public = key.verifying_key().to_sec1_point(false);
        Ok(Self { key, public })
    }

    /// Set the serial number and key identifier explicitly, without rcgen/crypto.
    pub fn certificate_params(&self, names: Vec<String>) -> Result<CertificateParams, String> {
        let mut params = CertificateParams::new(names).map_err(|e| e.to_string())?;
        let mut serial = <[u8; 20]>::try_generate().map_err(|e| e.to_string())?;
        // Positive, nonzero, at most 20 octets (RFC 5280 section 4.1.2.2).
        serial[0] = (serial[0] & 0x7f) | 1;
        params.serial_number = Some(rcgen::SerialNumber::from_slice(&serial));
        params.key_identifier_method =
            KeyIdMethod::PreSpecified(Sha256::digest(self.subject_public_key_info()).to_vec());
        Ok(params)
    }

    /// PKCS#8 private key, as used by rustls and existing saved certificates.
    pub fn serialize_der(&self) -> Result<Vec<u8>, p256::pkcs8::Error> {
        Ok(self.key.to_pkcs8_der()?.as_bytes().to_vec())
    }

    /// PEM-encoded PKCS#8 private key.
    pub fn serialize_pem(&self) -> Result<String, p256::pkcs8::Error> {
        Ok(self.key.to_pkcs8_pem(LineEnding::LF)?.to_string())
    }
}

impl PublicKeyData for KeyPair {
    fn der_bytes(&self) -> &[u8] {
        self.public.as_bytes()
    }

    fn algorithm(&self) -> &'static SignatureAlgorithm {
        &rcgen::PKCS_ECDSA_P256_SHA256
    }
}

impl rcgen::SigningKey for KeyPair {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        let signature: Signature = self
            .key
            .try_sign(msg)
            .map_err(|_| rcgen::Error::RemoteKeyError)?;
        Ok(signature.to_der().as_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::{VerifyingKey, signature::Verifier};
    use p256::pkcs8::DecodePrivateKey;
    use x509_parser::prelude::FromDer;

    #[test]
    fn private_key_formats_and_der_signatures() {
        let key = KeyPair::generate().unwrap();
        let der = key.serialize_der().unwrap();
        let restored = SigningKey::from_pkcs8_der(&der).unwrap();
        let pem = SigningKey::from_pkcs8_pem(&key.serialize_pem().unwrap()).unwrap();
        assert_eq!(restored.verifying_key(), key.key.verifying_key());
        assert_eq!(pem.verifying_key(), key.key.verifying_key());
        let signature = rcgen::SigningKey::sign(&key, b"certificate bytes").unwrap();
        let signature = Signature::from_der(&signature).unwrap();
        restored
            .verifying_key()
            .verify(b"certificate bytes", &signature)
            .unwrap();
        assert!(
            restored
                .verifying_key()
                .verify(b"altered bytes", &signature)
                .is_err()
        );
        assert!(
            KeyPair::generate()
                .unwrap()
                .key
                .verifying_key()
                .verify(b"certificate bytes", &signature)
                .is_err()
        );
    }

    #[test]
    fn certificate_metadata_and_signature() {
        let key = KeyPair::generate().unwrap();
        let names = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
        let mut params = key.certificate_params(names.clone()).unwrap();
        let other = key.certificate_params(names).unwrap();
        assert_ne!(params.serial_number, other.serial_number);
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        params.not_before = rcgen::date_time_ymd(2025, 1, 1);
        params.not_after = rcgen::date_time_ymd(2030, 1, 1);
        let certificate = params.self_signed(&key).unwrap();
        let (rest, parsed) =
            x509_parser::certificate::X509Certificate::from_der(certificate.der()).unwrap();
        assert!(rest.is_empty());
        assert_eq!(
            parsed.signature_algorithm.algorithm.to_id_string(),
            "1.2.840.10045.4.3.2"
        );
        assert_eq!(parsed.raw_serial().len(), 20);
        assert_eq!(parsed.validity().not_before.timestamp(), 1_735_689_600);
        assert_eq!(parsed.validity().not_after.timestamp(), 1_893_456_000);
        let san = parsed.subject_alternative_name().unwrap().unwrap();
        assert_eq!(san.value.general_names.len(), 3);
        assert!(matches!(
            &san.value.general_names[0],
            x509_parser::extensions::GeneralName::DNSName("localhost")
        ));
        assert!(
            matches!(&san.value.general_names[1], x509_parser::extensions::GeneralName::IPAddress(bytes) if *bytes == [127, 0, 0, 1])
        );
        assert!(
            parsed
                .extended_key_usage()
                .unwrap()
                .unwrap()
                .value
                .server_auth
        );
        let verifying =
            VerifyingKey::from_sec1_bytes(parsed.public_key().subject_public_key.data.as_ref())
                .unwrap();
        let signature = Signature::from_der(parsed.signature_value.data.as_ref()).unwrap();
        verifying
            .verify(parsed.tbs_certificate.as_ref(), &signature)
            .unwrap();
    }
}
