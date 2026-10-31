//! Forward proxy: CONNECT with MITM or tunneling.
//!
//! Serves a forward proxy that terminates TLS for hosts with a matching rule,
//! rewrites the placeholder credential, enforces GitHub policy, and
//! encrypts upstream again. Unmatched hosts are tunneled opaquely.

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

use crate::copilot::{CopilotError, CopilotTokens};
use crate::rewrite::rewrite_header;

/// The boxed error type the gateway uses across its public boundary.
///
/// This is a nominal wrapper rather than a type alias because `Box<dyn Error>`
/// type aliases trip rustc's higher ranked lifetime limitation when a future
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

/// Serve the forward proxy on `addr` until the process exits, trusting only
/// the platform root store for upstream connections.
pub async fn serve(
    addr: SocketAddr,
    table: Arc<TokenTable>,
    authority: Arc<Authority>,
) -> Result<(), BoxError> {
    serve_with_extra_ca(addr, table, authority, None).await
}

/// Serve the forward proxy on `addr` until the process exits, optionally
/// trusting an extra certificate authority for upstream connections in tests.
pub async fn serve_with_extra_ca(
    addr: SocketAddr,
    table: Arc<TokenTable>,
    authority: Arc<Authority>,
    extra_ca: Option<Arc<Authority>>,
) -> Result<(), BoxError> {
    let roots = Arc::new(upstream_root_store(extra_ca.as_deref()));
    let mut clients = upstream_clients(roots);
    if let Some(configuration) = table.copilot() {
        let configuration = configuration.clone();
        let upstream = clients.clone();
        clients.copilot = Some(Arc::new(CopilotTokens::new(Arc::new(move || {
            let upstream = upstream.clone();
            let configuration = configuration.clone();
            Box::pin(async move {
                let result = async {
                    let token = configuration
                        .oauth_token
                        .as_deref()
                        .ok_or(CopilotError::MissingOauth)?;
                    let account = host_json_with_clients(
                        &upstream,
                        "GET",
                        "https://api.github.com/copilot_internal/user",
                        Some(token),
                        None,
                    )
                    .await?;
                    crate::copilot::CopilotSession::account(token, account)
                }
                .await;
                if let Err(error) = &result {
                    tracing::warn!(credential = %configuration.credential, "{error}");
                }
                result
            })
        }))));
    }
    let acceptor = TlsAcceptor::from(mitm_server_config(Arc::clone(&authority)));

    let listener = TcpListener::bind(addr).await?;
    tracing::info!("listening on {}", listener.local_addr()?);

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(err) => {
                tracing::warn!("accept failed: {err}");
                if is_resource_exhaustion(&err) {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                continue;
            }
        };
        let table = Arc::clone(&table);
        let acceptor = acceptor.clone();
        let clients = clients.clone();

        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, table, acceptor, clients).await {
                tracing::debug!("connection from {peer} failed: {err}");
            }
        });
    }
}

fn is_resource_exhaustion(err: &std::io::Error) -> bool {
    matches!(err.kind(), std::io::ErrorKind::OutOfMemory)
        || matches!(err.raw_os_error(), Some(12 | 23 | 24 | 105))
}

/// Handle one client connection: the proxy control channel over plain HTTP.
async fn handle_connection(
    stream: TcpStream,
    table: Arc<TokenTable>,
    acceptor: TlsAcceptor,
    clients: UpstreamClients,
) -> Result<(), BoxError> {
    let service = service_fn(move |req| {
        let table = Arc::clone(&table);
        let acceptor = acceptor.clone();
        let clients = clients.clone();
        async move { proxy_request(req, table, acceptor, clients).await }
    });

    http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades()
        .await?;

    Ok(())
}

/// Route one proxy request: CONNECT or an absolute form HTTP request.
async fn proxy_request(
    req: Request<Incoming>,
    table: Arc<TokenTable>,
    acceptor: TlsAcceptor,
    clients: UpstreamClients,
) -> Result<Response<BoxBody>, BoxError> {
    if req.method() == Method::CONNECT {
        handle_connect(req, table, acceptor, clients).await
    } else {
        handle_absolute_form(req).await
    }
}

/// Handle a CONNECT request, choosing MITM for matched hosts and tunneling the rest.
async fn handle_connect(
    mut req: Request<Incoming>,
    table: Arc<TokenTable>,
    acceptor: TlsAcceptor,
    clients: UpstreamClients,
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

    let matched = table.matching_rule(&lowercase_host).cloned().or_else(|| {
        crate::copilot::is_copilot_host(&lowercase_host).then(|| Rule {
            hosts: vec![lowercase_host.clone()],
            auth_header: "authorization".to_string(),
            placeholder: crate::copilot::PLACEHOLDER.to_string(),
            secret: String::new(),
            policy: None,
        })
    });
    if let Some(rule) = matched {
        let on_upgrade = hyper::upgrade::on(&mut req);
        tokio::spawn(async move {
            match on_upgrade.await {
                Ok(upgraded) => {
                    if let Err(err) = mitm(upgraded, target, rule, acceptor, clients).await {
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
    acceptor: TlsAcceptor,
    clients: UpstreamClients,
) -> Result<(), BoxError> {
    let tls = acceptor.accept(TokioIo::new(upgraded)).await?;

    let service = service_fn(move |req| {
        let rule = rule.clone();
        let target = target.clone();
        let clients = clients.clone();
        async move {
            handle_inner_request(
                req.map(|body: Incoming| body.map_err(into_box).boxed()),
                rule,
                target,
                clients,
            )
            .await
        }
    });

    auto::Builder::new(TokioExecutor::new())
        .serve_connection(TokioIo::new(tls), service)
        .await?;

    Ok(())
}

/// Handle one decrypted request from the client, rewriting and enforcing policy.
async fn handle_inner_request(
    mut req: Request<BoxBody>,
    rule: Rule,
    target: String,
    clients: UpstreamClients,
) -> Result<Response<BoxBody>, BoxError> {
    let host = authority_host(&target)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let path = req.uri().path();
    if (host == "api.github.com" || host.ends_with(".github.com"))
        && (path == "/copilot_internal" || path.starts_with("/copilot_internal/"))
    {
        return Ok(forbidden());
    }
    if crate::copilot::is_copilot_host(&host) {
        let authorization = req
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok());
        let placeholder = authorization.is_some_and(|value| {
            rewrite_header(value, crate::copilot::PLACEHOLDER, "validated").is_some()
        });
        if placeholder {
            let Some(tokens) = &clients.copilot else {
                return Ok(forbidden());
            };
            let session = match tokens.session().await {
                Ok(session) => session,
                Err(error) => {
                    tracing::warn!("Copilot authentication failed: {error}");
                    return Ok(Response::builder()
                        .status(StatusCode::BAD_GATEWAY)
                        .body(full_body(
                            "Copilot authentication failed. Check the host gateway log.",
                        ))
                        .expect("valid response"));
                }
            };
            set_header(
                &mut req,
                "authorization",
                format!("Bearer {}", session.token),
            )?;
            if req
                .headers()
                .get("x-api-key")
                .is_some_and(|value| value == crate::copilot::PLACEHOLDER)
            {
                req.headers_mut().remove("x-api-key");
            }
            let endpoint: Uri = session.endpoint.parse().map_err(BoxError::from_error)?;
            let target = endpoint
                .authority()
                .expect("validated endpoint")
                .as_str()
                .to_string();
            set_header(&mut req, "host", target.clone())?;
            return forward_over_tls(req, target, clients).await;
        }
    }
    let Some(rewritten) = rewrite_request_header(&req, &rule) else {
        return forward_over_tls(req, target, clients).await;
    };

    if let Some(policy) = &rule.policy {
        let path_and_query = req
            .uri()
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or_else(|| req.uri().path());
        if policy.evaluate(req.method().as_str(), path_and_query) == Decision::Deny {
            tracing::warn!("policy denied {} {}", req.method(), path_and_query);
            return Ok(forbidden());
        }
    }

    set_header(&mut req, &rule.auth_header, rewritten)?;
    forward_over_tls(req, target, clients).await
}

/// Return the rewritten header value, or `None` when the header is absent or
/// carries no placeholder.
fn rewrite_request_header(req: &Request<BoxBody>, rule: &Rule) -> Option<String> {
    let value = req.headers().get(rule.auth_header.as_str())?;
    let value = value.to_str().ok()?;
    rewrite_header(value, &rule.placeholder, &rule.secret)
}

/// Replace the value of `name` on `req` with `value`.
fn set_header(req: &mut Request<BoxBody>, name: &str, value: String) -> Result<(), BoxError> {
    let name = HeaderName::from_bytes(name.as_bytes())?;
    let value = HeaderValue::from_str(&value)?;
    req.headers_mut().insert(name, value);
    Ok(())
}

/// Forward a decrypted request upstream over TLS, selecting the upstream
/// protocol from the request version so it matches the MITM leg protocol.
async fn forward_over_tls(
    req: Request<BoxBody>,
    target: String,
    clients: UpstreamClients,
) -> Result<Response<BoxBody>, BoxError> {
    let mut req = req;
    *req.uri_mut() = absolute_uri(&target, req.uri())?;
    let client = match req.version() {
        http::Version::HTTP_2 => &clients.http2,
        _ => &clients.http1,
    };
    let response = client.request(req).await.map_err(BoxError::from_error)?;
    Ok(response.map(|body| body.map_err(into_box).boxed()))
}

/// Convert an origin form URI into absolute form for `https://{authority}`.
fn absolute_uri(authority: &str, uri: &Uri) -> Result<Uri, BoxError> {
    let path_and_query = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    http::Uri::builder()
        .scheme("https")
        .authority(authority)
        .path_and_query(path_and_query)
        .build()
        .map_err(BoxError::from_error)
}

/// The upstream HTTP client, which negotiates the protocol offered over ALPN.
type UpstreamClient = hyper_util::client::legacy::Client<UpstreamConnector, BoxBody>;

/// The upstream clients, one per supported protocol version, so the upstream
/// connection matches the request version.
#[derive(Clone)]
struct UpstreamClients {
    http1: UpstreamClient,
    http2: UpstreamClient,
    copilot: Option<Arc<CopilotTokens>>,
}

/// Build the HTTP/1.1 and HTTP/2 upstream clients sharing a root store.
fn upstream_clients(roots: Arc<rustls::RootCertStore>) -> UpstreamClients {
    UpstreamClients {
        http1: upstream_client(Arc::clone(&roots), &[b"http/1.1"]),
        http2: upstream_client(roots, &[b"h2"]),
        copilot: None,
    }
}

pub async fn host_json(
    method: &str,
    endpoint: &str,
    token: Option<&str>,
    payload: Option<serde_json::Value>,
) -> Result<serde_json::Value, CopilotError> {
    let clients = upstream_clients(Arc::new(upstream_root_store(None)));
    host_json_with_clients(&clients, method, endpoint, token, payload).await
}

async fn host_json_with_clients(
    clients: &UpstreamClients,
    method: &str,
    endpoint: &str,
    token: Option<&str>,
    payload: Option<serde_json::Value>,
) -> Result<serde_json::Value, CopilotError> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut request = Request::builder()
            .method(method)
            .uri(endpoint)
            .header("accept", "application/json")
            .header("user-agent", "aegis/0.1.0")
            .header("x-github-api-version", "2025-04-01");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let body = if let Some(payload) = payload {
            request = request.header("content-type", "application/json");
            Full::new(Bytes::from(
                serde_json::to_vec(&payload).map_err(|_| CopilotError::InvalidResponse)?,
            ))
            .map_err(|never| match never {})
            .boxed()
        } else {
            empty_body()
        };
        let request = request
            .body(body)
            .map_err(|_| CopilotError::InvalidResponse)?;
        let response = clients
            .http1
            .request(request)
            .await
            .map_err(|_| CopilotError::Transport)?;
        if !response.status().is_success() {
            return Err(CopilotError::Status(response.status().as_u16()));
        }
        let bytes = http_body_util::Limited::new(response.into_body(), 64 * 1024)
            .collect()
            .await
            .map_err(|_| CopilotError::InvalidResponse)?
            .to_bytes();
        serde_json::from_slice(&bytes).map_err(|_| CopilotError::InvalidResponse)
    })
    .await
    .map_err(|_| CopilotError::Transport)?
}

/// Build one upstream client offering the given ALPN protocols.
fn upstream_client(roots: Arc<rustls::RootCertStore>, alpn: &[&[u8]]) -> UpstreamClient {
    let connector = UpstreamConnector::new(upstream_tls_connector(roots, alpn));
    hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(connector)
}

/// A connector that opens a TLS connection to the request authority and
/// reports the negotiated ALPN protocol so the client speaks HTTP/2 when the
/// upstream selects it.
#[derive(Clone)]
struct UpstreamConnector {
    tls: Arc<TlsConnector>,
    #[cfg(test)]
    address: Option<SocketAddr>,
}

impl UpstreamConnector {
    fn new(tls: Arc<TlsConnector>) -> Self {
        Self {
            tls,
            #[cfg(test)]
            address: None,
        }
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
        #[cfg(test)]
        let address = self.address;
        Box::pin(async move {
            let authority = dst.authority().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "uri has no authority")
            })?;
            let target = upstream_target(&dst)?;
            let host = authority.host().to_string();

            #[cfg(test)]
            let target = address.map(|address| address.to_string()).unwrap_or(target);
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

fn upstream_target(uri: &Uri) -> Result<String, std::io::Error> {
    let authority = uri
        .authority()
        .ok_or_else(|| std::io::Error::other("URI has no authority"))?;
    Ok(if authority.port_u16().is_some() {
        authority.as_str().to_string()
    } else {
        format!("{}:443", authority.as_str())
    })
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

/// Handle an absolute form request by forwarding it over plain HTTP.
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

/// Convert an absolute form URI into origin form.
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

/// Build the root store for upstream connections from the platform trust store
/// and an optional extra certificate authority used in tests.
fn upstream_root_store(extra_ca: Option<&Authority>) -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore::empty();

    if let Some(authority) = extra_ca {
        let mut pem = std::io::Cursor::new(authority.ca_pem());
        for certificate in rustls_pemfile::certs(&mut pem).flatten() {
            let _ = roots.add(certificate);
        }
    }

    // Platform roots so real upstreams verify.
    let native = rustls_native_certs::load_native_certs();
    let _ = roots.add_parsable_certificates(native.certs);

    roots
}

/// The TLS connector the gateway uses when encrypting upstream again, offering
/// the given ALPN protocols.
fn upstream_tls_connector(roots: Arc<rustls::RootCertStore>, alpn: &[&[u8]]) -> Arc<TlsConnector> {
    let provider = rustls::crypto::ring::default_provider();
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .expect("supported protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();

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

    #[tokio::test]
    async fn copilot_routes_oauth_and_preserves_streamed_requests() {
        let directory = temp_dir("copilot");
        let authority = Authority::load_or_create(&directory).unwrap();
        let certified = authority
            .leaf_certified_key("api.enterprise.githubcopilot.com")
            .unwrap();
        let configuration = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(FixedResolver { certified }));
        let acceptor = TlsAcceptor::from(Arc::new(configuration));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, mut received) = tokio::sync::mpsc::channel(8);
        let server = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                let sender = sender.clone();
                tokio::spawn(async move {
                    let stream = acceptor.accept(stream).await.unwrap();
                    let service = service_fn(move |request: Request<Incoming>| {
                        let sender = sender.clone();
                        async move {
                            let (parts, body) = request.into_parts();
                            let bytes = body.collect().await.unwrap().to_bytes();
                            sender
                                .send((parts.uri, parts.headers, bytes))
                                .await
                                .unwrap();
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .header("content-type", "text/event-stream")
                                    .body(Full::new(Bytes::from_static(
                                        b"data: public-response\n\n",
                                    )))
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        let roots = Arc::new(upstream_root_store(Some(&authority)));
        let mut connector =
            UpstreamConnector::new(upstream_tls_connector(roots.clone(), &[b"http/1.1"]));
        connector.address = Some(address);
        let client =
            hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(connector);
        let tokens = Arc::new(CopilotTokens::new(Arc::new(|| {
            Box::pin(async {
                crate::copilot::CopilotSession::account(
                    "oauth-host-sentinel",
                    serde_json::json!({"chat_enabled":true,
                        "endpoints":{"api":"https://api.enterprise.githubcopilot.com"}}),
                )
            })
        })));
        let clients = UpstreamClients {
            http1: client.clone(),
            http2: client,
            copilot: Some(tokens),
        };
        for path in ["/models", "/chat/completions", "/responses", "/v1/messages"] {
            let rule = Rule {
                hosts: vec!["api.githubcopilot.com".into()],
                auth_header: "authorization".into(),
                placeholder: crate::copilot::PLACEHOLDER.into(),
                secret: String::new(),
                policy: None,
            };
            let request = Request::builder()
                .method("POST")
                .uri(format!("{path}?test=1"))
                .header("authorization", "Bearer copilot-aegis-placeholder")
                .header("x-api-key", "copilot-aegis-placeholder")
                .header("x-interaction-type", "conversation-agent")
                .body(full_body("request-body"))
                .unwrap();
            let response = handle_inner_request(
                request,
                rule,
                "api.githubcopilot.com:443".into(),
                clients.clone(),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.into_body().collect().await.unwrap().to_bytes(),
                "data: public-response\n\n"
            );
            let (uri, headers, body) = received.recv().await.unwrap();
            assert_eq!(
                uri.path_and_query().unwrap().as_str(),
                format!("{path}?test=1")
            );
            assert_eq!(headers["authorization"], "Bearer oauth-host-sentinel");
            assert_eq!(headers["host"], "api.enterprise.githubcopilot.com");
            assert_eq!(headers["x-interaction-type"], "conversation-agent");
            assert!(!headers.contains_key("x-api-key"));
            assert_eq!(body, "request-body");
        }
        server.abort();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn host_https_requests_supply_default_port() {
        assert_eq!(
            upstream_target(
                &"https://api.github.com/copilot_internal/user"
                    .parse()
                    .unwrap()
            )
            .unwrap(),
            "api.github.com:443"
        );
        assert_eq!(
            upstream_target(&"https://localhost:8443/models".parse().unwrap()).unwrap(),
            "localhost:8443"
        );
    }

    #[tokio::test]
    async fn guest_token_endpoint_is_denied_without_placeholder() {
        let clients = upstream_clients(Arc::new(rustls::RootCertStore::empty()));
        for authorization in [
            None,
            Some("Bearer unrelated"),
            Some("Bearer placeholder-secret"),
        ] {
            let mut request = Request::builder().uri("/copilot_internal/v2/token?client=test");
            if let Some(value) = authorization {
                request = request.header("authorization", value);
            }
            let response = handle_inner_request(
                request.body(empty_body()).unwrap(),
                rule_for("api.github.com"),
                "api.github.com:443".to_string(),
                clients.clone(),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
    }

    #[tokio::test]
    async fn disabled_copilot_does_not_forward_placeholder() {
        let clients = upstream_clients(Arc::new(rustls::RootCertStore::empty()));
        let mut rule = rule_for("api.githubcopilot.com");
        rule.policy = None;
        let request = Request::builder()
            .uri("/models")
            .header("authorization", "Bearer copilot-aegis-placeholder")
            .body(empty_body())
            .unwrap();
        let response = handle_inner_request(
            request,
            rule,
            "api.githubcopilot.com:443".to_string(),
            clients,
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

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

    /// Start a local upstream that advertises both `h2` and `http/1.1` over
    /// ALPN and whose response body echoes the request version and the
    /// authorization header.
    async fn start_dual_upstream(authority: &Authority) -> SocketAddr {
        let certified = authority.leaf_certified_key("localhost").expect("leaf");
        let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("supported protocol versions")
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(FixedResolver { certified }));
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(config));

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");

        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.expect("accept");
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let tls = acceptor.accept(stream).await.expect("tls accept");
                    serve_dual_echo(tls).await;
                });
            }
        });

        addr
    }

    /// Serve HTTP/1.1 or HTTP/2 requests, echoing the request version and the
    /// authorization header in the body.
    async fn serve_dual_echo(tls: TlsStream<TcpStream>) {
        let service = service_fn(|req: Request<Incoming>| async move {
            let authorization = req
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            let body = format!("version={:?} auth={authorization}", req.version());
            Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from(body))))
        });

        let _ = auto::Builder::new(TokioExecutor::new())
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
    fn production_upstream_root_store_excludes_gateway_ca() {
        let dir = temp_dir("roots");
        let authority = Authority::load_or_create(&dir).expect("authority");

        let gateway_certs = rustls_pemfile::certs(&mut std::io::Cursor::new(authority.ca_pem()))
            .flatten()
            .count();

        let production_store = upstream_root_store(None);
        let test_store = upstream_root_store(Some(&authority));

        assert_eq!(
            test_store.roots.len(),
            production_store.roots.len() + gateway_certs,
            "production upstream root store must not trust the gateway CA"
        );
    }

    #[tokio::test]
    async fn matched_host_with_placeholder_is_rewritten() {
        let dir = temp_dir("rewrite");
        let authority = Arc::new(Authority::load_or_create(&dir).expect("authority"));
        let ca_pem = authority.ca_pem();

        let upstream_addr = start_upstream(&authority).await;
        let table = Arc::new(TokenTable::new(vec![rule_for("localhost")]));

        let proxy_addr = ephemeral_addr().await;
        tokio::spawn(serve_with_extra_ca(
            proxy_addr,
            table,
            Arc::clone(&authority),
            Some(Arc::clone(&authority)),
        ));

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
        tokio::spawn(serve_with_extra_ca(
            proxy_addr,
            table,
            Arc::clone(&authority),
            Some(Arc::clone(&authority)),
        ));

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
        tokio::spawn(serve_with_extra_ca(
            proxy_addr,
            table,
            Arc::clone(&authority),
            Some(Arc::clone(&authority)),
        ));

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
        tokio::spawn(serve_with_extra_ca(
            proxy_addr,
            table,
            Arc::clone(&authority),
            Some(Arc::clone(&authority)),
        ));

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

    #[tokio::test]
    async fn matched_host_over_h1_to_h2_capable_upstream_is_rewritten() {
        let dir = temp_dir("h1-to-h2-upstream");
        let authority = Arc::new(Authority::load_or_create(&dir).expect("authority"));
        let ca_pem = authority.ca_pem();

        let upstream_addr = start_dual_upstream(&authority).await;
        let table = Arc::new(TokenTable::new(vec![rule_for("localhost")]));

        let proxy_addr = ephemeral_addr().await;
        tokio::spawn(serve_with_extra_ca(
            proxy_addr,
            table,
            Arc::clone(&authority),
            Some(Arc::clone(&authority)),
        ));

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
        assert!(text.contains("version=HTTP/1.1"), "response: {text}");
        assert!(!text.contains("version=HTTP/2"), "response: {text}");
        assert!(text.contains(SECRET), "response: {text}");
        assert!(!text.contains(PLACEHOLDER), "response: {text}");
    }
}
