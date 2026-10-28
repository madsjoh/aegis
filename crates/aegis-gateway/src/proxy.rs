//! Forward proxy: CONNECT with MITM or tunneling.
//!
//! Serves a forward proxy that terminates TLS for hosts with a matching rule,
//! rewrites the placeholder credential, enforces GitHub policy, and
//! re-encrypts upstream. Unmatched hosts are tunneled opaquely.

use std::error::Error;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::header::{HeaderName, HeaderValue};
use http::Uri;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use rustls::pki_types::ServerName;
use rustls::server::ResolvesServerCert;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::TlsConnector;
use tower_service::Service;

use aegis_core::ca::Authority;
use aegis_core::policy::Decision;
use aegis_core::token_table::{Rule, TokenTable};

use crate::rewrite::rewrite_header;

/// The boxed error type the gateway uses across its public boundary.
///
/// This is a nominal wrapper rather than a type alias because `Box<dyn Error>`
/// type aliases trip rustc's higher-ranked lifetime limitation when a future
/// that returns them is required to be `Send`, as it is here by `tokio::spawn`.
pub struct BoxError(Box<dyn Error + Send + Sync>);

impl BoxError {
    /// Box any error into a [`BoxError`].
    pub fn from_error<E>(error: E) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        BoxError(Box::new(error))
    }
}

impl std::fmt::Display for BoxError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::fmt::Debug for BoxError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<BoxError> for Box<dyn Error + Send + Sync> {
    fn from(error: BoxError) -> Self {
        error.0
    }
}

macro_rules! from_error {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl From<$ty> for BoxError {
                fn from(error: $ty) -> Self {
                    BoxError(Box::new(error))
                }
            }
        )+
    };
}

from_error!(
    std::io::Error,
    hyper::Error,
    rustls::pki_types::InvalidDnsNameError,
    http::header::InvalidHeaderName,
    http::header::InvalidHeaderValue,
);

impl From<Box<dyn Error + Send + Sync>> for BoxError {
    fn from(error: Box<dyn Error + Send + Sync>) -> Self {
        BoxError(error)
    }
}

type BoxBody = http_body_util::combinators::BoxBody<Bytes, BoxError>;

/// Serve the forward proxy on `addr` until the process exits.
pub async fn serve(
    addr: SocketAddr,
    table: Arc<TokenTable>,
    authority: Arc<Authority>,
) -> Result<(), BoxError> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("listening on {}", listener.local_addr()?);

    loop {
        let (stream, peer) = listener.accept().await?;
        let table = Arc::clone(&table);
        let authority = Arc::clone(&authority);

        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, table, authority).await {
                tracing::debug!("connection from {peer} failed: {err}");
            }
        });
    }
}

/// Handle one client connection: the proxy control channel over plain HTTP.
async fn handle_connection(
    stream: TcpStream,
    table: Arc<TokenTable>,
    authority: Arc<Authority>,
) -> Result<(), BoxError> {
    let service = service_fn(move |req| {
        let table = Arc::clone(&table);
        let authority = Arc::clone(&authority);
        async move { proxy_request(req, table, authority).await }
    });

    http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades()
        .await?;

    Ok(())
}

/// Route one proxy request: CONNECT or an absolute-form HTTP request.
async fn proxy_request(
    req: Request<Incoming>,
    table: Arc<TokenTable>,
    authority: Arc<Authority>,
) -> Result<Response<BoxBody>, BoxError> {
    if req.method() == Method::CONNECT {
        handle_connect(req, table, authority).await
    } else {
        handle_absolute_form(req).await
    }
}

/// Handle a CONNECT request, choosing MITM for matched hosts and tunneling the rest.
async fn handle_connect(
    mut req: Request<Incoming>,
    table: Arc<TokenTable>,
    authority: Arc<Authority>,
) -> Result<Response<BoxBody>, BoxError> {
    let Some(target) = req
        .uri()
        .authority()
        .map(|authority| authority.as_str().to_string())
    else {
        return Ok(bad_request());
    };
    let Some(host) = authority_host(&target) else {
        return Ok(bad_request());
    };

    let lowercase_host = host.to_ascii_lowercase();

    if let Some(rule) = table.matching_rule(&lowercase_host) {
        let rule = rule.clone();
        let on_upgrade = hyper::upgrade::on(&mut req);
        let authority = Arc::clone(&authority);
        tokio::spawn(async move {
            match on_upgrade.await {
                Ok(upgraded) => {
                    if let Err(err) = mitm(upgraded, target, rule, authority).await {
                        tracing::debug!("mitm connection failed: {err}");
                    }
                }
                Err(err) => tracing::debug!("upgrade failed: {err}"),
            }
        });
        return Ok(connection_established());
    }

    let upstream = match TcpStream::connect(&target).await {
        Ok(stream) => stream,
        Err(err) => {
            tracing::debug!("upstream connect to {target} failed: {err}");
            return Ok(bad_gateway());
        }
    };

    let on_upgrade = hyper::upgrade::on(&mut req);
    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                let mut client = TokioIo::new(upgraded);
                let mut upstream = upstream;
                if let Err(err) = tokio::io::copy_bidirectional(&mut client, &mut upstream).await {
                    tracing::debug!("tunnel to {target} closed: {err}");
                }
            }
            Err(err) => tracing::debug!("upgrade failed: {err}"),
        }
    });

    Ok(connection_established())
}

/// Terminate TLS for a matched host and serve the decrypted HTTP/1.1 or
/// HTTP/2 requests.
async fn mitm(
    upgraded: hyper::upgrade::Upgraded,
    target: String,
    rule: Rule,
    authority: Arc<Authority>,
) -> Result<(), BoxError> {
    let acceptor = TlsAcceptor::from(mitm_server_config(Arc::clone(&authority)));
    let tls = acceptor.accept(TokioIo::new(upgraded)).await?;

    let client = upstream_client(&authority);

    let service = service_fn(move |req| {
        let rule = rule.clone();
        let target = target.clone();
        let client = client.clone();
        async move { handle_inner_request(req, rule, target, client).await }
    });

    auto::Builder::new(TokioExecutor::new())
        .serve_connection(TokioIo::new(tls), service)
        .await?;

    Ok(())
}

/// Handle one decrypted request from the client, rewriting and enforcing policy.
async fn handle_inner_request(
    mut req: Request<Incoming>,
    rule: Rule,
    target: String,
    client: UpstreamClient,
) -> Result<Response<BoxBody>, BoxError> {
    let Some(rewritten) = rewrite_request_header(&req, &rule) else {
        return forward_over_tls(req, target, client).await;
    };

    if let Some(policy) = &rule.policy {
        if policy.evaluate(req.method().as_str(), req.uri().path()) == Decision::Deny {
            tracing::warn!("policy denied {} {}", req.method(), req.uri().path());
            return Ok(forbidden());
        }
    }

    set_header(&mut req, &rule.auth_header, rewritten)?;
    forward_over_tls(req, target, client).await
}

/// Return the rewritten header value, or `None` when the header is absent or
/// carries no placeholder.
fn rewrite_request_header(req: &Request<Incoming>, rule: &Rule) -> Option<String> {
    let value = req.headers().get(rule.auth_header.as_str())?;
    let value = value.to_str().ok()?;
    rewrite_header(value, &rule.placeholder, &rule.secret)
}

/// Replace the value of `name` on `req` with `value`.
fn set_header(req: &mut Request<Incoming>, name: &str, value: String) -> Result<(), BoxError> {
    let name = HeaderName::from_bytes(name.as_bytes())?;
    let value = HeaderValue::from_str(&value)?;
    req.headers_mut().insert(name, value);
    Ok(())
}

/// Forward a decrypted request upstream over TLS, negotiating HTTP/2 or
/// HTTP/1.1 through ALPN.
async fn forward_over_tls(
    req: Request<Incoming>,
    target: String,
    client: UpstreamClient,
) -> Result<Response<BoxBody>, BoxError> {
    let mut req = req;
    *req.uri_mut() = absolute_uri(&target, req.uri())?;
    let response = client
        .request(req)
        .await
        .map_err(BoxError::from_error)?;
    Ok(response.map(|body| body.map_err(into_box).boxed()))
}

/// Convert an origin-form URI into absolute form for `https://{authority}`.
fn absolute_uri(authority: &str, uri: &Uri) -> Result<Uri, BoxError> {
    let path_and_query = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    http::Uri::builder()
        .scheme("https")
        .authority(authority)
        .path_and_query(path_and_query)
        .build()
        .map_err(BoxError::from_error)
}

/// The upstream HTTP client, which negotiates HTTP/1.1 or HTTP/2 through ALPN.
type UpstreamClient = hyper_util::client::legacy::Client<UpstreamConnector, Incoming>;

/// Build the client that forwards decrypted requests upstream.
fn upstream_client(authority: &Authority) -> UpstreamClient {
    let connector = UpstreamConnector::new(upstream_tls_connector(authority));
    hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(connector)
}

/// A connector that opens a TLS connection to the request authority and
/// reports the negotiated ALPN protocol so the client speaks HTTP/2 when the
/// upstream selects it.
#[derive(Clone)]
struct UpstreamConnector {
    tls: Arc<TlsConnector>,
}

impl UpstreamConnector {
    fn new(tls: Arc<TlsConnector>) -> Self {
        Self { tls }
    }
}

impl Service<Uri> for UpstreamConnector {
    type Response = UpstreamIo;
    type Error = BoxError;
    type Future = Pin<Box<dyn std::future::Future<Output = Result<UpstreamIo, BoxError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        let tls = Arc::clone(&self.tls);
        Box::pin(async move {
            let authority = dst.authority().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "uri has no authority")
            })?;
            let target = authority.as_str().to_string();
            let host = authority.host().to_string();

            let stream = TcpStream::connect(&target).await?;
            let server_name = server_name(&host)?;
            let tls_stream = tls.connect(server_name, stream).await?;
            let negotiated_h2 = tls_stream.get_ref().1.alpn_protocol() == Some(b"h2");

            Ok(UpstreamIo {
                inner: TokioIo::new(tls_stream),
                negotiated_h2,
            })
        })
    }
}

/// The upstream transport, which reports the negotiated protocol to the client.
struct UpstreamIo {
    inner: TokioIo<tokio_rustls::client::TlsStream<TcpStream>>,
    negotiated_h2: bool,
}

impl Connection for UpstreamIo {
    fn connected(&self) -> Connected {
        let connected = Connected::new();
        if self.negotiated_h2 {
            connected.negotiated_h2()
        } else {
            connected
        }
    }
}

impl hyper::rt::Read for UpstreamIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl hyper::rt::Write for UpstreamIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
}

/// Handle an absolute-form request by forwarding it over plain HTTP.
async fn handle_absolute_form(mut req: Request<Incoming>) -> Result<Response<BoxBody>, BoxError> {
    let Some(authority) = req.uri().authority() else {
        return Ok(bad_request());
    };
    let host = authority.host();
    let port = authority.port_u16().unwrap_or(80);
    let target = format!("{host}:{port}");

    let stream = TcpStream::connect(&target).await?;
    *req.uri_mut() = origin_form(req.uri());
    send_request_over(req, stream).await
}

/// Handshake an HTTP/1.1 client connection and forward one request over it.
async fn send_request_over<T>(req: Request<Incoming>, io: T) -> Result<Response<BoxBody>, BoxError>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(io);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let response = sender.send_request(req).await?;
    Ok(response.map(|body| body.map_err(into_box).boxed()))
}

/// Convert a `hyper::Error` into the crate error type.
fn into_box(err: hyper::Error) -> BoxError {
    BoxError::from(err)
}

/// Convert an absolute-form URI into origin-form.
fn origin_form(uri: &Uri) -> Uri {
    uri.path_and_query()
        .map(|path_and_query| path_and_query.as_str())
        .unwrap_or("/")
        .parse()
        .expect("path and query is a valid URI")
}

/// Extract the host from a `host:port` authority, supporting bracketed IPv6.
fn authority_host(authority: &str) -> Option<String> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        return Some(rest[..end].to_string());
    }
    authority.rsplit_once(':').map(|(host, _)| host.to_string())
}

/// Build a [`ServerName`] from a host, allowing IP addresses.
fn server_name(host: &str) -> Result<ServerName<'static>, BoxError> {
    if let Ok(address) = host.parse::<std::net::IpAddr>() {
        return Ok(ServerName::IpAddress(address.into()));
    }
    Ok(ServerName::try_from(host.to_string())?)
}

/// The rustls server configuration that mints a leaf for the client's SNI name.
fn mitm_server_config(authority: Arc<Authority>) -> Arc<rustls::ServerConfig> {
    let provider = rustls::crypto::ring::default_provider();
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .expect("supported protocol versions")
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(CertificateResolver { authority }));
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Arc::new(config)
}

/// A server certificate resolver that mints leaves through the [`Authority`].
struct CertificateResolver {
    authority: Arc<Authority>,
}

impl std::fmt::Debug for CertificateResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CertificateResolver")
            .finish_non_exhaustive()
    }
}

impl ResolvesServerCert for CertificateResolver {
    fn resolve(
        &self,
        client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let hostname = client_hello.server_name()?;
        self.authority.leaf_certified_key(hostname).ok()
    }
}

/// Build the root store for upstream connections: the gateway CA plus the
/// platform trust store.
fn upstream_root_store(authority: &Authority) -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore::empty();

    // The gateway CA so gateway-signed local upstreams verify.
    let mut pem = std::io::Cursor::new(authority.ca_pem());
    for certificate in rustls_pemfile::certs(&mut pem).flatten() {
        let _ = roots.add(certificate);
    }

    // Platform roots so real upstreams verify. Loading is best-effort: an empty
    // or unreadable store simply means only gateway-signed upstreams verify.
    let native = rustls_native_certs::load_native_certs();
    let _ = roots.add_parsable_certificates(native.certs);

    roots
}

/// The TLS connector the gateway uses when re-encrypting upstream.
///
/// The gateway CA is trusted so that a local upstream minted by the same
/// authority verifies in tests, and the platform trust store is trusted so
/// real upstreams verify too.
fn upstream_tls_connector(authority: &Authority) -> Arc<TlsConnector> {
    let provider = rustls::crypto::ring::default_provider();
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .expect("supported protocol versions")
        .with_root_certificates(upstream_root_store(authority))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Arc::new(TlsConnector::from(Arc::new(config)))
}

fn connection_established() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::OK)
        .body(empty_body())
        .expect("valid response")
}

fn forbidden() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .body(full_body("Forbidden"))
        .expect("valid response")
}

fn bad_gateway() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(full_body("Bad Gateway"))
        .expect("valid response")
}

fn bad_request() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(full_body("Bad Request"))
        .expect("valid response")
}

fn empty_body() -> BoxBody {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

fn full_body(text: &'static str) -> BoxBody {
    Full::new(Bytes::from_static(text.as_bytes()))
        .map_err(|never| match never {})
        .boxed()
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::Arc;

    use aegis_core::policy::GithubPolicy;
    use aegis_core::token_table::Rule;
    use hyper::client::conn::http2 as client_http2;
    use hyper::server::conn::http2 as server_http2;
    use rustls::server::ResolvesServerCert;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::server::TlsStream;
    use tokio_rustls::TlsAcceptor;

    use super::*;

    const PLACEHOLDER: &str = "placeholder-secret";
    const SECRET: &str = "real-token";

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aegis-proxy-{name}-{}-{}",
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

    fn rule_for(host: &str) -> Rule {
        Rule {
            hosts: vec![host.to_string()],
            auth_header: "authorization".to_string(),
            placeholder: PLACEHOLDER.to_string(),
            secret: SECRET.to_string(),
            policy: Some(GithubPolicy::new(vec![])),
        }
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

    /// Start a local HTTPS upstream whose certificate is signed by the gateway
    /// CA and whose response body echoes the request head.
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
    async fn echo_request(mut tls: TlsStream<TcpStream>) {
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

    /// Start a local HTTP/2 upstream that advertises `h2` over ALPN and whose
    /// response body echoes the request authorization header.
    async fn start_h2_upstream(authority: &Authority) -> SocketAddr {
        let certified = authority.leaf_certified_key("localhost").expect("leaf");
        let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("supported protocol versions")
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(FixedResolver { certified }));
        config.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(config));

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");

        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.expect("accept");
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let tls = acceptor.accept(stream).await.expect("tls accept");
                    serve_h2_echo(tls).await;
                });
            }
        });

        addr
    }

    /// Serve HTTP/2 requests and echo the authorization header in the body.
    async fn serve_h2_echo(tls: TlsStream<TcpStream>) {
        let service = service_fn(|req: Request<Incoming>| async move {
            let authorization = req
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from(authorization))))
        });

        let _ = server_http2::Builder::new(TokioExecutor::new())
            .serve_connection(TokioIo::new(tls), service)
            .await;
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

    /// Drive a CONNECT request through the proxy and return the full TLS
    /// response bytes after the tunnel or interception.
    async fn connect_via_proxy(
        proxy_addr: SocketAddr,
        authority: &str,
        server_name: &str,
        ca_pem: &[u8],
        request: &[u8],
    ) -> Vec<u8> {
        let mut stream = connect_with_retry(proxy_addr).await;

        let connect = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n");
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
        let server_name = ServerName::try_from(server_name.to_string()).expect("server name");
        let mut tls = connector
            .connect(server_name, stream)
            .await
            .expect("tls connect");

        tls.write_all(request).await.expect("write request");

        let mut response = Vec::new();
        tls.read_to_end(&mut response).await.expect("read response");
        response
    }

    #[test]
    fn upstream_root_store_seeds_gateway_ca_and_platform_roots() {
        let dir = temp_dir("roots");
        let authority = Authority::load_or_create(&dir).expect("authority");

        let gateway_certs = rustls_pemfile::certs(&mut std::io::Cursor::new(authority.ca_pem()))
            .flatten()
            .count();

        let native = rustls_native_certs::load_native_certs();

        let store = upstream_root_store(&authority);

        // The gateway CA is always present, so gateway-signed upstreams verify.
        assert!(
            store.roots.len() >= gateway_certs,
            "gateway CA missing from the upstream root store"
        );

        // Platform roots, when the host has any, are loaded too.
        if !native.certs.is_empty() {
            assert!(
                store.roots.len() > gateway_certs,
                "platform roots missing from the upstream root store"
            );
        }
    }

    #[tokio::test]
    async fn matched_host_with_placeholder_is_rewritten() {
        let dir = temp_dir("rewrite");
        let authority = Arc::new(Authority::load_or_create(&dir).expect("authority"));
        let ca_pem = authority.ca_pem();

        let upstream_addr = start_upstream(&authority).await;
        let table = Arc::new(TokenTable::new(vec![rule_for("localhost")]));

        let proxy_addr = ephemeral_addr().await;
        tokio::spawn(serve(proxy_addr, table, Arc::clone(&authority)));

        let target = format!("localhost:{}", upstream_addr.port());
        let request = format!(
            "GET /echo HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {PLACEHOLDER}\r\nConnection: close\r\n\r\n"
        );
        let response = connect_via_proxy(
            proxy_addr,
            &target,
            "localhost",
            &ca_pem,
            request.as_bytes(),
        )
        .await;
        let text = String::from_utf8_lossy(&response);

        assert!(text.starts_with("HTTP/1.1 200"), "response: {text}");
        assert!(text.contains(SECRET), "response: {text}");
        assert!(!text.contains(PLACEHOLDER), "response: {text}");
    }

    #[tokio::test]
    async fn matched_host_without_placeholder_is_forwarded_unmodified() {
        let dir = temp_dir("no-placeholder");
        let authority = Arc::new(Authority::load_or_create(&dir).expect("authority"));
        let ca_pem = authority.ca_pem();

        let upstream_addr = start_upstream(&authority).await;
        let table = Arc::new(TokenTable::new(vec![rule_for("localhost")]));

        let proxy_addr = ephemeral_addr().await;
        tokio::spawn(serve(proxy_addr, table, Arc::clone(&authority)));

        // A POST is denied by the empty policy, so a 200 here proves the policy
        // is skipped when the request carries no placeholder.
        let target = format!("localhost:{}", upstream_addr.port());
        let request = b"POST /echo HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer other-token\r\nConnection: close\r\n\r\n";
        let response = connect_via_proxy(proxy_addr, &target, "localhost", &ca_pem, request).await;
        let text = String::from_utf8_lossy(&response);

        assert!(text.starts_with("HTTP/1.1 200"), "response: {text}");
        assert!(text.contains("other-token"), "response: {text}");
        assert!(!text.contains(SECRET), "response: {text}");
    }

    #[tokio::test]
    async fn unmatched_host_is_tunneled_byte_for_byte() {
        let dir = temp_dir("tunnel");
        let authority = Arc::new(Authority::load_or_create(&dir).expect("authority"));
        let ca_pem = authority.ca_pem();

        let upstream_addr = start_upstream(&authority).await;
        let table = Arc::new(TokenTable::new(vec![rule_for("localhost")]));

        let proxy_addr = ephemeral_addr().await;
        tokio::spawn(serve(proxy_addr, table, Arc::clone(&authority)));

        // `127.0.0.1` has no rule, so the proxy tunnels. The placeholder must
        // reach the upstream untouched, proving no interception happened.
        let target = format!("127.0.0.1:{}", upstream_addr.port());
        let request = format!(
            "GET /echo HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {PLACEHOLDER}\r\nConnection: close\r\n\r\n"
        );
        let response = connect_via_proxy(
            proxy_addr,
            &target,
            "localhost",
            &ca_pem,
            request.as_bytes(),
        )
        .await;
        let text = String::from_utf8_lossy(&response);

        assert!(text.starts_with("HTTP/1.1 200"), "response: {text}");
        assert!(text.contains("Host: localhost"), "response: {text}");
        assert!(text.contains(PLACEHOLDER), "response: {text}");
        assert!(!text.contains(SECRET), "response: {text}");
    }

    #[tokio::test]
    async fn matched_host_over_h2_is_rewritten() {
        let dir = temp_dir("h2");
        let authority = Arc::new(Authority::load_or_create(&dir).expect("authority"));
        let ca_pem = authority.ca_pem();

        let upstream_addr = start_h2_upstream(&authority).await;
        let table = Arc::new(TokenTable::new(vec![rule_for("localhost")]));

        let proxy_addr = ephemeral_addr().await;
        tokio::spawn(serve(proxy_addr, table, Arc::clone(&authority)));

        let target = format!("localhost:{}", upstream_addr.port());

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
        let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("supported protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];
        let connector = TlsConnector::from(Arc::new(config));
        let server_name = ServerName::try_from("localhost".to_string()).expect("server name");
        let tls = connector
            .connect(server_name, stream)
            .await
            .expect("tls connect");
        assert_eq!(
            tls.get_ref().1.alpn_protocol(),
            Some(&b"h2"[..]),
            "MITM leg did not negotiate h2"
        );

        let (mut sender, conn) = client_http2::Builder::new(TokioExecutor::new())
            .handshake::<_, Empty<Bytes>>(TokioIo::new(tls))
            .await
            .expect("h2 handshake");
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let request = Request::builder()
            .method(Method::GET)
            .uri(format!("https://{target}/echo"))
            .header("authorization", format!("Bearer {PLACEHOLDER}"))
            .body(Empty::<Bytes>::new())
            .expect("request");
        let response = sender.send_request(request).await.expect("send request");
        let body = response
            .into_body()
            .collect()
            .await
            .expect("read body")
            .to_bytes();

        let text = String::from_utf8_lossy(&body);
        assert!(text.contains(SECRET), "response: {text}");
        assert!(!text.contains(PLACEHOLDER), "response: {text}");
    }
}
