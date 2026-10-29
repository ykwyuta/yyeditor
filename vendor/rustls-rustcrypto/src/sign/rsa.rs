#[cfg(feature = "alloc")]
use alloc::{boxed::Box, format, string::ToString, sync::Arc};

use pkcs8::DecodePrivateKey;
use pki_types::PrivateKeyDer;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1::EncodeRsaPublicKey;
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;
use rustls::sign::{Signer, SigningKey};
use rustls::{SignatureAlgorithm, SignatureScheme};
use sha2::{Sha256, Sha384, Sha512};

const ALL_RSA_SCHEMES: &[SignatureScheme] = &[
    SignatureScheme::RSA_PSS_SHA512,
    SignatureScheme::RSA_PSS_SHA384,
    SignatureScheme::RSA_PSS_SHA256,
    SignatureScheme::RSA_PKCS1_SHA512,
    SignatureScheme::RSA_PKCS1_SHA384,
    SignatureScheme::RSA_PKCS1_SHA256,
];

pub struct RsaSigningKey(RsaPrivateKey);

impl core::fmt::Debug for RsaSigningKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RsaSigningKey").finish_non_exhaustive()
    }
}

impl TryFrom<&PrivateKeyDer<'_>> for RsaSigningKey {
    type Error = rustls::Error;

    fn try_from(value: &PrivateKeyDer<'_>) -> Result<Self, Self::Error> {
        let pkey = match value {
            PrivateKeyDer::Pkcs8(der) => RsaPrivateKey::from_pkcs8_der(der.secret_pkcs8_der())
                .map_err(|e| format!("failed to decrypt private key: {e}")),
            PrivateKeyDer::Pkcs1(der) => RsaPrivateKey::from_pkcs1_der(der.secret_pkcs1_der())
                .map_err(|e| format!("failed to decrypt private key: {e}")),
            PrivateKeyDer::Sec1(_) => Err("RSA does not support SEC1 key".to_string()),
            _ => Err("not supported".into()),
        };

        let key = pkey.map_err(rustls::Error::General)?;
        key.validate()
            .map_err(|_| rustls::Error::General("invalid RSA private key".into()))?;
        if !(2048..=8192).contains(&key.n().bits()) {
            return Err(rustls::Error::General(
                "RSA key must be 2048..8192 bits".into(),
            ));
        }
        Ok(Self(key))
    }
}

impl SigningKey for RsaSigningKey {
    fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
        ALL_RSA_SCHEMES
            .iter()
            .find(|scheme| offered.contains(scheme))
            .and_then(|&scheme| {
                macro_rules! signer {
                    ($key:ty) => {{
                        Some(Box::new(super::GenericRandomizedSigner {
                            _marker: Default::default(),
                            key: Arc::new(<$key>::new(self.0.clone())),
                            scheme,
                        }) as Box<_>)
                    }};
                }

                match scheme {
                    SignatureScheme::RSA_PSS_SHA512 => {
                        signer! {rsa::pss::BlindedSigningKey::<Sha512>}
                    }
                    SignatureScheme::RSA_PSS_SHA384 => {
                        signer! {rsa::pss::BlindedSigningKey::<Sha384>}
                    }
                    SignatureScheme::RSA_PSS_SHA256 => {
                        signer! {rsa::pss::BlindedSigningKey::<Sha256>}
                    }
                    SignatureScheme::RSA_PKCS1_SHA512 => {
                        signer! {rsa::pkcs1v15::SigningKey::<Sha512>}
                    }
                    SignatureScheme::RSA_PKCS1_SHA384 => {
                        signer! {rsa::pkcs1v15::SigningKey::<Sha384>}
                    }
                    SignatureScheme::RSA_PKCS1_SHA256 => {
                        signer! {rsa::pkcs1v15::SigningKey::<Sha256>}
                    }
                    _ => None,
                }
            })
    }

    fn algorithm(&self) -> SignatureAlgorithm {
        SignatureAlgorithm::RSA
    }

    fn public_key(&self) -> Option<pki_types::SubjectPublicKeyInfoDer<'_>> {
        let key = self.0.to_public_key().to_pkcs1_der().ok()?;
        Some(rustls::sign::public_key_to_spki(
            &pki_types::alg_id::RSA_ENCRYPTION,
            key.as_bytes(),
        ))
    }
}
