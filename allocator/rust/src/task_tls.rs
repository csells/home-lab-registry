use std::sync::Arc;

use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, Error, RootCertStore, SignatureScheme,
    client::{
        WebPkiServerVerifier,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        verify_server_name,
    },
    pki_types::{CertificateDer, ServerName, UnixTime, pem::PemObject},
    server::ParsedCertificate,
};
use x509_cert::{
    Certificate,
    der::{Decode, asn1::ObjectIdentifier},
    ext::pkix::ExtendedKeyUsage,
};

pub(crate) fn client_config(pem: &[u8]) -> Result<ClientConfig, &'static str> {
    let mut roots = RootCertStore::empty();
    let pins = CertificateDer::pem_slice_iter(pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Invalid allocator CA file")?;
    for pin in &pins {
        roots
            .add(pin.clone())
            .map_err(|_| "Invalid allocator CA file")?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .map_err(|_| "Invalid allocator CA file")?;
    Ok(ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| "Unable to configure allocator TLS")?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AllocatorIdentity { verifier, pins }))
        .with_no_client_auth())
}

#[derive(Debug)]
struct AllocatorIdentity {
    verifier: Arc<WebPkiServerVerifier>,
    pins: Vec<CertificateDer<'static>>,
}

impl ServerCertVerifier for AllocatorIdentity {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let result = self
            .verifier
            .verify_server_cert(end_entity, intermediates, name, ocsp, now);
        let legacy_ca = matches!(
            &result,
            Err(Error::InvalidCertificate(CertificateError::Other(error)))
                if matches!(error.0.downcast_ref::<webpki::Error>(), Some(webpki::Error::CaUsedAsEndEntity))
        );
        if !legacy_ca
            || !intermediates.is_empty()
            || !self
                .pins
                .iter()
                .any(|pin| pin.as_ref() == end_entity.as_ref())
        {
            return result;
        }
        // Older brokers installed their exact self-issued TLS certificate as
        // the trust file. Honor that endpoint pin, including name, lifetime,
        // and server use, without accepting a different CA as a leaf.
        let certificate = Certificate::from_der(end_entity.as_ref())
            .map_err(|_| CertificateError::BadEncoding)?;
        let info = certificate.tbs_certificate();
        if info.issuer() != info.subject() {
            return result;
        }
        let validity = info.validity();
        let before = validity.not_before.to_unix_duration().as_secs();
        let after = validity.not_after.to_unix_duration().as_secs();
        if now.as_secs() < before {
            return Err(CertificateError::NotValidYet.into());
        }
        if now.as_secs() > after {
            return Err(CertificateError::Expired.into());
        }
        const SERVER_AUTH: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.1");
        const ANY_USAGE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.37.0");
        let usages = info
            .get_extension::<ExtendedKeyUsage>()
            .map_err(|_| CertificateError::BadEncoding)?;
        if usages.is_some_and(|(_, usage)| {
            !usage.0.contains(&SERVER_AUTH) && !usage.0.contains(&ANY_USAGE)
        }) {
            return Err(CertificateError::InvalidPurpose.into());
        }
        verify_server_name(&ParsedCertificate::try_from(end_entity)?, name)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.verifier
            .verify_tls12_signature(message, cert, signature)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.verifier
            .verify_tls13_signature(message, cert, signature)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.verifier.supported_verify_schemes()
    }
}
