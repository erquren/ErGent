use super::*;
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::PrivateKeyDer;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn pem_bundle_adds_every_certificate() {
    let mut roots = RootCertStore::empty();
    let pem = [
        std::fs::read(fixture("ca.pem")).unwrap(),
        std::fs::read(fixture("other-ca.pem")).unwrap(),
    ]
    .concat();
    add_ca_certificates(&mut roots, &pem).unwrap();
    assert_eq!(roots.len(), 2);
}

#[test]
fn empty_malformed_and_non_certificate_pem_fail_closed() {
    for pem in [
        b"".as_slice(),
        b"this is not a PEM certificate",
        b"-----BEGIN CERTIFICATE-----\n!invalid!\n-----END CERTIFICATE-----\n",
        b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
        b"-----BEGIN CERTIFICATE-----\nAAAA\n",
        include_bytes!("../../tests/fixtures/server-key.pem"),
    ] {
        assert!(add_ca_certificates(&mut RootCertStore::empty(), pem).is_err());
    }
    // Do not silently keep the good prefix of a malformed bundle or accept a key.
    for suffix in [
        b"-----BEGIN CERTIFICATE-----\ninvalid\n".as_slice(),
        include_bytes!("../../tests/fixtures/server-key.pem"),
    ] {
        let pem = [std::fs::read(fixture("ca.pem")).unwrap(), suffix.to_vec()].concat();
        assert!(add_ca_certificates(&mut RootCertStore::empty(), &pem).is_err());
    }
}

#[test]
fn explicit_ca_path_is_absolute_and_missing_or_unreadable_files_fail() {
    let cert = fixture("ca.pem");
    let cwd = std::env::current_dir().unwrap();
    let relative = cert.strip_prefix(&cwd).unwrap();
    assert!(relative.is_relative());
    let tls = AgentTls::new(Some(relative)).unwrap();
    assert_eq!(
        tls.ca_cert(),
        Some(fixture("ca.pem").canonicalize().unwrap().as_path())
    );
    assert!(AgentTls::new(Some(&fixture("does-not-exist.pem"))).is_err());
    assert!(AgentTls::new(Some(&fixture("."))).is_err());
}

#[derive(Clone, Copy)]
enum Protocol {
    Https,
    Wss,
}

async fn attempt(certificate: &str, ca: Option<&str>, protocol: Protocol) -> bool {
    let cert = CertificateDer::from_pem_file(fixture(certificate)).unwrap();
    let key = PrivateKeyDer::from_pem_file(fixture("server-key.pem")).unwrap();
    let server = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert], key)
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server));
        let Ok(mut stream) = acceptor.accept(socket).await else {
            return;
        };
        match protocol {
            Protocol::Https => {
                let mut request = [0u8; 4096];
                if stream.read(&mut request).await.is_ok() {
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                        )
                        .await;
                    let _ = stream.shutdown().await;
                }
            }
            Protocol::Wss => {
                if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                    let _ = ws
                        .send(tokio_tungstenite::tungstenite::Message::Text(
                            "verified".into(),
                        ))
                        .await;
                    let _ = ws.close(None).await;
                }
            }
        }
    });
    let path = ca.map(fixture);
    let tls = AgentTls::new(path.as_deref()).unwrap();
    let success = match protocol {
        Protocol::Https => tls
            .http_client()
            .unwrap()
            .get(format!("https://{address}/api/v1/agent/enroll"))
            .send()
            .await
            .is_ok(),
        Protocol::Wss => {
            match tokio_tungstenite::connect_async_tls_with_config(
                format!("wss://{address}/api/v1/ws/agent"),
                None,
                false,
                Some(tls.websocket_connector()),
            )
            .await
            {
                Ok((mut ws, _)) => ws.next().await.is_some_and(|m| m.is_ok()),
                Err(_) => false,
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), peer)
        .await
        .expect("local TLS test peer timed out")
        .unwrap();
    success
}

#[tokio::test]
async fn https_and_wss_share_trust_and_verify_chain_expiry_and_ip_san() {
    for protocol in [Protocol::Https, Protocol::Wss] {
        assert!(
            attempt("server.pem", Some("ca.pem"), protocol).await,
            "valid custom CA must work"
        );
        assert!(
            !attempt("server.pem", None, protocol).await,
            "untrusted private CA must fail"
        );
        assert!(
            !attempt("server.pem", Some("other-ca.pem"), protocol).await,
            "wrong CA must fail"
        );
        assert!(
            !attempt("wrong-ip.pem", Some("ca.pem"), protocol).await,
            "wrong IP SAN must fail"
        );
        assert!(
            !attempt("expired.pem", Some("ca.pem"), protocol).await,
            "expired server certificate must fail"
        );
    }
}

#[test]
fn native_trust_is_used_for_both_transports_without_ca_option() {
    const CHILD_ENV: &str = "ERGENT_TEST_NATIVE_CA_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        // Isolate trust-store environment changes from concurrent Rust tests.
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tls::tests::native_trust_is_used_for_both_transports_without_ca_option",
            ])
            .env(CHILD_ENV, "1")
            .env("SSL_CERT_FILE", fixture("ca.pem"))
            .env("SSL_CERT_DIR", fixture("no-system-cert-directory"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        for protocol in [Protocol::Https, Protocol::Wss] {
            assert!(attempt("server.pem", None, protocol).await);
            assert!(!attempt("wrong-ip.pem", None, protocol).await);
        }
    });
}
