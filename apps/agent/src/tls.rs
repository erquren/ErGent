//! Shared certificate verification for enrollment HTTPS and the daemon's WSS connection.
use anyhow::{ensure, Context, Result};
use rustls::{
    pki_types::{
        pem::{PemObject, SectionKind},
        CertificateDer,
    },
    ClientConfig, RootCertStore,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub(crate) struct AgentTls {
    ca_cert: Option<PathBuf>,
    config: Arc<ClientConfig>,
}

impl AgentTls {
    pub(crate) fn new(ca_cert: Option<&Path>) -> Result<Self> {
        let mut roots = RootCertStore::empty();
        // Keep public roots available even on minimal systems, and also honor the
        // platform trust store (including organization-managed private CAs).
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let native = rustls_native_certs::load_native_certs();
        for error in native.errors {
            tracing::warn!(%error, "无法加载部分系统 CA，仍使用公共 CA 和显式 CA");
        }
        let (_, ignored) = roots.add_parsable_certificates(native.certs);
        if ignored != 0 {
            tracing::warn!(ignored, "忽略无法解析的系统 CA");
        }

        let ca_cert = ca_cert
            .map(|path| -> Result<PathBuf> {
                let absolute = path
                    .canonicalize()
                    .with_context(|| format!("无法读取 CA 证书文件 {}", path.display()))?;
                let pem = std::fs::read(&absolute)
                    .with_context(|| format!("无法读取 CA 证书文件 {}", absolute.display()))?;
                add_ca_certificates(&mut roots, &pem)
                    .with_context(|| format!("CA 证书文件无效：{}", absolute.display()))?;
                Ok(absolute)
            })
            .transpose()?;

        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()?
                .with_root_certificates(roots)
                .with_no_client_auth();
        Ok(Self {
            ca_cert,
            config: Arc::new(config),
        })
    }

    pub(crate) fn ca_cert(&self) -> Option<&Path> {
        self.ca_cert.as_deref()
    }

    pub(crate) fn http_client(&self) -> Result<reqwest::Client> {
        Ok(reqwest::Client::builder()
            .use_preconfigured_tls((*self.config).clone())
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()?)
    }

    pub(crate) fn websocket_connector(&self) -> tokio_tungstenite::Connector {
        tokio_tungstenite::Connector::Rustls(self.config.clone())
    }
}

fn add_ca_certificates(roots: &mut RootCertStore, pem: &[u8]) -> Result<()> {
    let mut count = 0;
    for item in <(SectionKind, Vec<u8>)>::pem_slice_iter(pem) {
        let (kind, der) = item.context("无法解析 PEM")?;
        ensure!(
            kind == SectionKind::Certificate,
            "--ca-cert 只能包含 PEM 证书，不能包含私钥或其他 PEM 项"
        );
        // Unlike add_parsable_certificates, add never silently ignores malformed DER.
        roots
            .add(CertificateDer::from(der))
            .context("无法解析 X.509 CA 证书")?;
        count += 1;
    }
    ensure!(count != 0, "--ca-cert 文件中没有 PEM CA 证书");
    Ok(())
}

#[cfg(test)]
mod tests;
