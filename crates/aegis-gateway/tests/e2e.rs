//! End-to-end integration test for the gateway `serve` API.
//!
//! Drives a real forward proxy through its public API against a local HTTPS
//! upstream whose certificate is signed by the same authority the gateway uses
//! to intercept, verifying credential rewriting and policy enforcement across
//! the full request path.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rustls::pki_types::ServerName;
use rustls::server::ResolvesServerCert;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::client::TlsStream;
use tokio_rustls::server::TlsStream as ServerTlsStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use aegis_core::ca::Authority;
use aegis_core::policy::{CompiledRepoRule, GithubPolicy, Scope};
use aegis_core::secrets::Permission;
use aegis_core::token_table::{Rule, TokenTable};
use aegis_gateway::proxy::serve;

const PLACEHOLDER: &str = "github_pat_aegis_placeholder";
const SECRET: &str = "github_pat_real_secret";

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "aegis-e2e-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

async fn ephemeral_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);
    addr
}

/// A certificate resolver that always returns the same certified key, so a
/// gateway signed leaf can serve as the local upstream certificate.
#[derive(Debug)]
struct FixedResolver {
    certified: Arc<rustls::sign::CertifiedKey>,
}

impl ResolvesServerCert for FixedResolver {
    fn resolve(
        &self,
        _client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(Arc::clone(&self.certified))
    }
}

/// Start a local HTTPS upstream whose certificate is signed by the gateway CA
/// and whose response body echoes the request head.
async fn start_upstream(authority: &Authority) -> SocketAddr {
    let certified = authority.leaf_certified_key("localhost").expect("leaf");
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("supported protocol versions")
    .with_no_client_auth()
    .with_cert_resolver(Arc::new(FixedResolver { certified }));
    let acceptor = TlsAcceptor::from(Arc::new(config));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");

    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.expect("accept");
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let tls = acceptor.accept(stream).await.expect("tls accept");
                echo_request(tls).await;
            });
        }
    });

    addr
}

/// Read one HTTP request head and echo it back as the response body.
async fn echo_request(mut tls: ServerTlsStream<TcpStream>) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while head.len() < 64 * 1024 {
        match tls.read(&mut byte).await {
            Ok(0) => return,
            Ok(_) => {
                head.push(byte[0]);
                if head.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return,
        }
    }

    let head_text = String::from_utf8_lossy(&head).into_owned();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        head_text.len(),
        head_text
    );
    let _ = tls.write_all(response.as_bytes()).await;
    let _ = tls.shutdown().await;
}

async fn read_until_double_crlf<R: AsyncRead + Unpin>(reader: &mut R, buf: &mut Vec<u8>) {
    let mut byte = [0u8; 1];
    loop {
        let count = reader.read(&mut byte).await.expect("read");
        if count == 0 {
            break;
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
}

/// Retry connecting until the freshly spawned proxy has bound its listener.
async fn connect_with_retry(addr: SocketAddr) -> TcpStream {
    for _ in 0..200 {
        match TcpStream::connect(addr).await {
            Ok(stream) => return stream,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
        }
    }
    panic!("proxy did not start listening on {addr}");
}

/// Establish a CONNECT tunnel through the proxy and complete the TLS handshake
/// with the gateway minted leaf for `localhost`, trusting the gateway CA.
async fn tls_to_proxy(
    proxy_addr: SocketAddr,
    target: &str,
    ca_pem: &[u8],
) -> TlsStream<TcpStream> {
    let mut stream = connect_with_retry(proxy_addr).await;

    let connect = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
    stream.write_all(connect.as_bytes()).await.expect("write");

    let mut head = Vec::new();
    read_until_double_crlf(&mut stream, &mut head).await;
    assert!(
        head.starts_with(b"HTTP/1.1 200"),
        "unexpected CONNECT response: {}",
        String::from_utf8_lossy(&head)
    );

    let mut roots = rustls::RootCertStore::empty();
    let mut ca_reader = std::io::Cursor::new(ca_pem);
    for certificate in rustls_pemfile::certs(&mut ca_reader).flatten() {
        roots.add(certificate).expect("root");
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("supported protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let server_name = ServerName::try_from("localhost".to_string()).expect("server name");
    connector
        .connect(server_name, stream)
        .await
        .expect("tls connect")
}

#[tokio::test]
async fn placeholder_is_rewritten_and_read_only_push_is_denied() {
    let dir = temp_dir("serve");
    let authority = Arc::new(Authority::load_or_create(&dir).expect("authority"));
    let ca_pem = authority.ca_pem();

    let upstream_addr = start_upstream(&authority).await;

    let policy = GithubPolicy::new(vec![CompiledRepoRule {
        pattern: "org/repo".to_string(),
        permissions: BTreeMap::from([(Scope::Contents, Permission::Read)]),
    }]);
    let rule = Rule {
        hosts: vec!["localhost".to_string()],
        auth_header: "authorization".to_string(),
        placeholder: PLACEHOLDER.to_string(),
        secret: SECRET.to_string(),
        policy: Some(policy),
    };
    let table = Arc::new(TokenTable::new(vec![rule]));

    let proxy_addr = ephemeral_addr().await;
    tokio::spawn(serve(proxy_addr, table, Arc::clone(&authority)));

    let target = format!("localhost:{}", upstream_addr.port());
    let basic = format!(
        "Basic {}",
        STANDARD.encode(format!("x-access-token:{PLACEHOLDER}"))
    );

    // A git-style read request carrying the placeholder must reach the upstream
    // with the placeholder replaced by the real token.
    let read_request = format!(
        "GET /org/repo.git/info/refs?service=git-upload-pack HTTP/1.1\r\n\
         Host: localhost\r\n\
         Authorization: {basic}\r\n\
         Connection: close\r\n\
         \r\n"
    );
    let mut tls = tls_to_proxy(proxy_addr, &target, &ca_pem).await;
    tls.write_all(read_request.as_bytes()).await.expect("write");
    let mut response = Vec::new();
    tls.read_to_end(&mut response).await.expect("read");
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 200"), "response: {text}");
    assert!(!text.contains(PLACEHOLDER), "response: {text}");

    let encoded = text
        .lines()
        .find_map(|line| line.strip_prefix("authorization: "))
        .and_then(|value| value.strip_prefix("Basic "))
        .expect("authorization header echoed");
    let credentials = String::from_utf8(STANDARD.decode(encoded.trim()).expect("basic decodes"))
        .expect("basic is utf-8");
    assert_eq!(credentials, format!("x-access-token:{SECRET}"));

    // A push to a repository whose policy grants only contents read must be
    // answered 403 by the gateway before any upstream is contacted.
    let push_request = format!(
        "POST /org/repo.git/git-receive-pack HTTP/1.1\r\n\
         Host: localhost\r\n\
         Authorization: {basic}\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\
         \r\n"
    );
    let mut tls = tls_to_proxy(proxy_addr, &target, &ca_pem).await;
    tls.write_all(push_request.as_bytes()).await.expect("write");
    let mut head = Vec::new();
    read_until_double_crlf(&mut tls, &mut head).await;
    let text = String::from_utf8_lossy(&head);
    assert!(text.starts_with("HTTP/1.1 403"), "response: {text}");
}
