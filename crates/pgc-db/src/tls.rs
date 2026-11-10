//! TLS for PostgreSQL connections (D-033): rustls with the `ring` provider.
//!
//! - `disable` / `prefer` / `require`: certificate not checked (libpq semantics);
//! - `verify-ca`: chain checked, host name not;
//! - `verify-full`: chain and host name checked.
//!
//! Trust anchors: the root certificate file of the connection if given (only that
//! file, as libpq), otherwise the OS certificate store.

use std::sync::{Arc, OnceLock};

use pgc_core::settings::SslMode;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::DbError;

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// OS trust store, loaded once per process (slow on some systems).
fn native_roots() -> &'static Result<Arc<RootCertStore>, String> {
    static ROOTS: OnceLock<Result<Arc<RootCertStore>, String>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let result = rustls_native_certs::load_native_certs();
        for e in &result.errors {
            tracing::warn!("OS certificate store: {e}");
        }
        let mut store = RootCertStore::empty();
        let (added, ignored) = store.add_parsable_certificates(result.certs);
        tracing::debug!("OS certificate store: {added} roots loaded, {ignored} ignored");
        if store.is_empty() {
            Err("no usable certificates in the OS certificate store".to_owned())
        } else {
            Ok(Arc::new(store))
        }
    })
}

fn roots(root_cert: Option<&str>) -> Result<Arc<RootCertStore>, DbError> {
    let Some(path) = root_cert else {
        return native_roots().clone().map_err(DbError::Other);
    };
    let err = |e: String| DbError::Other(format!("root certificate file {path}: {e}"));
    let data = std::fs::read(path).map_err(|e| err(e.to_string()))?;
    let mut store = RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut data.as_slice()) {
        let cert = cert.map_err(|e| err(e.to_string()))?;
        store.add(cert).map_err(|e| err(e.to_string()))?;
    }
    if store.is_empty() {
        return Err(err("no certificates found".to_owned()));
    }
    Ok(Arc::new(store))
}

/// TLS connector for a connection with `mode`.
pub fn connector(mode: SslMode, root_cert: Option<&str>) -> Result<MakeRustlsConnect, DbError> {
    let provider = provider();
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| DbError::Other(format!("TLS setup: {e}")))?;
    let config = match mode {
        SslMode::Disable | SslMode::Prefer | SslMode::Require => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertCheck(provider)))
            .with_no_client_auth(),
        SslMode::VerifyCa | SslMode::VerifyFull => {
            let verifier = WebPkiServerVerifier::builder_with_provider(roots(root_cert)?, provider)
                .build()
                .map_err(|e| DbError::Other(format!("TLS setup: {e}")))?;
            let verifier: Arc<dyn ServerCertVerifier> = if mode == SslMode::VerifyCa {
                Arc::new(IgnoreHostName(verifier))
            } else {
                verifier
            };
            builder
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth()
        }
    };
    Ok(MakeRustlsConnect::new(config))
}

/// Accepts any certificate, but still checks the handshake signatures.
#[derive(Debug)]
struct NoCertCheck(Arc<CryptoProvider>);

impl ServerCertVerifier for NoCertCheck {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
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
            &self.0.signature_verification_algorithms,
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
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// `verify-ca`: full chain verification, host name mismatch accepted.
#[derive(Debug)]
struct IgnoreHostName(Arc<WebPkiServerVerifier>);

impl ServerCertVerifier for IgnoreHostName {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self
            .0
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) => Ok(ServerCertVerified::assertion()),
            other => other,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connectors_build_for_unverified_modes() {
        for mode in [SslMode::Disable, SslMode::Prefer, SslMode::Require] {
            assert!(connector(mode, None).is_ok());
        }
    }

    #[test]
    fn missing_root_cert_file_is_an_error() {
        let Err(e) = connector(SslMode::VerifyFull, Some("/does/not/exist.pem")) else {
            panic!("expected an error");
        };
        assert!(e.to_string().contains("root certificate file"), "{e}");
    }
}
