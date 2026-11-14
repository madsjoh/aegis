//! Resolve GitHub global node IDs to repository owner and name.
//!
//! Turns a GitHub global node ID into the owner and name of the repository it
//! references, plus the subject kind for issue and pull request nodes. Results
//! are cached by ID so each node resolves at most once.

use std::collections::HashMap;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use tokio::sync::Mutex;

use aegis_core::graphql::NodeKind;

use crate::proxy::{BoxBody, UpstreamClient};

/// The kind of subject a subject node references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjectKind {
    Issue,
    PullRequest,
}

/// A resolved GitHub node: the repository it belongs to and, for subject
/// nodes, whether it is an issue or a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedNode {
    pub owner: String,
    pub name: String,
    pub subject_kind: Option<SubjectKind>,
}

/// An error produced while resolving a node ID.
#[derive(Debug, thiserror::Error)]
pub enum NodeResolutionError {
    #[error("node ID could not be resolved")]
    Resolution,
    #[error("node ID resolution failed: {0}")]
    Transport(String),
}

/// The GraphQL query that resolves a repository node to its name with owner.
const REPOSITORY_QUERY: &str = "\
query NodeRepository($id: ID!) {
  node(id: $id) {
    ... on Repository {
      nameWithOwner
    }
  }
}";

/// The GraphQL query that resolves a subject node to its type and repository.
const SUBJECT_QUERY: &str = "\
query NodeSubject($id: ID!) {
  node(id: $id) {
    __typename
    ... on Issue {
      repository {
        nameWithOwner
      }
    }
    ... on PullRequest {
      repository {
        nameWithOwner
      }
    }
  }
}";

/// Resolves GitHub global node IDs to repository owner and name, caching each
/// result by ID so a node is resolved at most once.
pub struct NodeResolver {
    client: UpstreamClient,
    cache: Mutex<HashMap<String, ResolvedNode>>,
}

impl NodeResolver {
    pub(crate) fn new(client: UpstreamClient) -> Self {
        Self {
            client,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub async fn resolve(
        &self,
        id: &str,
        kind: NodeKind,
        token: &str,
    ) -> Result<ResolvedNode, NodeResolutionError> {
        let mut cache = self.cache.lock().await;
        if let Some(resolved) = cache.get(id) {
            return Ok(resolved.clone());
        }

        let response = self.query(id, kind, token).await?;
        let resolved = match kind {
            NodeKind::Repository => parse_repository(&response)?,
            NodeKind::Subject => parse_subject(&response)?,
        };

        cache.insert(id.to_string(), resolved.clone());
        Ok(resolved)
    }

    /// POST the resolution query to the GitHub GraphQL API and return the JSON
    /// response body.
    async fn query(
        &self,
        id: &str,
        kind: NodeKind,
        token: &str,
    ) -> Result<serde_json::Value, NodeResolutionError> {
        let query = match kind {
            NodeKind::Repository => REPOSITORY_QUERY,
            NodeKind::Subject => SUBJECT_QUERY,
        };
        let body = serde_json::json!({
            "query": query,
            "variables": { "id": id },
        });
        let body = serde_json::to_vec(&body)
            .map_err(|error| NodeResolutionError::Transport(error.to_string()))?;

        let request = http::Request::builder()
            .method(http::Method::POST)
            .uri("https://api.github.com/graphql")
            .header("authorization", format!("Bearer {token}"))
            .header("accept", "application/json")
            .header("user-agent", "aegis/0.1.0")
            .header("content-type", "application/json")
            .body(full_body(body))
            .map_err(|error| NodeResolutionError::Transport(error.to_string()))?;

        let response = self
            .client
            .request(request)
            .await
            .map_err(|error| NodeResolutionError::Transport(error.to_string()))?;

        if !response.status().is_success() {
            return Err(NodeResolutionError::Transport(format!(
                "HTTP status {}",
                response.status().as_u16()
            )));
        }

        let bytes = http_body_util::Limited::new(response.into_body(), 64 * 1024)
            .collect()
            .await
            .map_err(|error| NodeResolutionError::Transport(error.to_string()))?
            .to_bytes();

        serde_json::from_slice(&bytes)
            .map_err(|error| NodeResolutionError::Transport(error.to_string()))
    }
}

/// Build a boxed response body from raw bytes.
fn full_body(body: Vec<u8>) -> BoxBody {
    Full::new(Bytes::from(body))
        .map_err(|never| match never {})
        .boxed()
}

/// Parse a repository node from a GraphQL response.
fn parse_repository(response: &serde_json::Value) -> Result<ResolvedNode, NodeResolutionError> {
    let node = response
        .pointer("/data/node")
        .ok_or(NodeResolutionError::Resolution)?;
    let name_with_owner = node
        .get("nameWithOwner")
        .and_then(|value| value.as_str())
        .ok_or(NodeResolutionError::Resolution)?;
    let (owner, name) = name_with_owner
        .split_once('/')
        .ok_or(NodeResolutionError::Resolution)?;
    Ok(ResolvedNode {
        owner: owner.to_string(),
        name: name.to_string(),
        subject_kind: None,
    })
}

/// Parse a subject node from a GraphQL response.
fn parse_subject(response: &serde_json::Value) -> Result<ResolvedNode, NodeResolutionError> {
    let node = response
        .pointer("/data/node")
        .ok_or(NodeResolutionError::Resolution)?;
    let subject_kind = match node
        .get("__typename")
        .and_then(|value| value.as_str())
    {
        Some("Issue") => SubjectKind::Issue,
        Some("PullRequest") => SubjectKind::PullRequest,
        _ => return Err(NodeResolutionError::Resolution),
    };
    let name_with_owner = node
        .pointer("/repository/nameWithOwner")
        .and_then(|value| value.as_str())
        .ok_or(NodeResolutionError::Resolution)?;
    let (owner, name) = name_with_owner
        .split_once('/')
        .ok_or(NodeResolutionError::Resolution)?;
    Ok(ResolvedNode {
        owner: owner.to_string(),
        name: name.to_string(),
        subject_kind: Some(subject_kind),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response, StatusCode};
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use rustls::server::ResolvesServerCert;
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::server::TlsStream;
    use tokio_rustls::TlsAcceptor;

    use aegis_core::ca::Authority;
    use aegis_core::graphql::NodeKind;

    use super::{NodeResolver, NodeResolutionError, SubjectKind};
    use crate::proxy::{upstream_root_store, upstream_tls_connector, UpstreamConnector};

    const REPOSITORY_RESPONSE: &str = r#"{"data":{"node":{"nameWithOwner":"org/repo"}}}"#;
    const ISSUE_RESPONSE: &str =
        r#"{"data":{"node":{"__typename":"Issue","repository":{"nameWithOwner":"org/repo"}}}}"#;
    const PULL_REQUEST_RESPONSE: &str =
        r#"{"data":{"node":{"__typename":"PullRequest","repository":{"nameWithOwner":"acme/widget"}}}}"#;
    const NULL_NODE_RESPONSE: &str = r#"{"data":{"node":null}}"#;

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

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aegis-nodes-{name}-{}-{}",
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

    /// Build a resolver whose upstream client talks to `address` over TLS,
    /// trusting `authority` as the root certificate authority.
    fn resolver_for(authority: &Authority, address: SocketAddr) -> NodeResolver {
        let roots = Arc::new(upstream_root_store(Some(authority)));
        let mut connector = UpstreamConnector::new(upstream_tls_connector(roots, &[b"http/1.1"]));
        connector.address = Some(address);
        let client =
            hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(connector);
        NodeResolver::new(client)
    }

    /// Start a local HTTPS upstream that answers GraphQL node lookups from a
    /// fixed table keyed by node ID and counts each request.
    async fn start_upstream(
        authority: &Authority,
        responses: Arc<std::sync::Mutex<HashMap<String, String>>>,
        requests: Arc<AtomicUsize>,
    ) -> SocketAddr {
        let certified = authority.leaf_certified_key("api.github.com").expect("leaf");
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
                let responses = Arc::clone(&responses);
                let requests = Arc::clone(&requests);
                tokio::spawn(async move {
                    let tls = acceptor.accept(stream).await.expect("tls accept");
                    serve_graphql(tls, responses, requests).await;
                });
            }
        });

        addr
    }

    /// Serve GraphQL requests by returning the canned response for the node ID
    /// in the request body.
    async fn serve_graphql(
        tls: TlsStream<TcpStream>,
        responses: Arc<std::sync::Mutex<HashMap<String, String>>>,
        requests: Arc<AtomicUsize>,
    ) {
        let service = service_fn(move |request: Request<Incoming>| {
            let responses = Arc::clone(&responses);
            let requests = Arc::clone(&requests);
            async move {
                requests.fetch_add(1, Ordering::SeqCst);
                let body = request
                    .into_body()
                    .collect()
                    .await
                    .expect("request body")
                    .to_bytes();
                let id = serde_json::from_slice::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|value| {
                        value
                            .pointer("/variables/id")
                            .and_then(|value| value.as_str())
                            .map(str::to_string)
                    });
                let payload = id
                    .as_deref()
                    .and_then(|id| responses.lock().unwrap().get(id).cloned())
                    .unwrap_or_else(|| NULL_NODE_RESPONSE.to_string());
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "application/json")
                        .body(Full::new(Bytes::from(payload)))
                        .expect("valid response"),
                )
            }
        });
        let _ = http1::Builder::new()
            .serve_connection(TokioIo::new(tls), service)
            .await;
    }

    #[tokio::test]
    async fn repository_node_resolves_to_owner_and_name() {
        let dir = temp_dir("repository");
        let authority = Authority::load_or_create(&dir).expect("authority");
        let responses = Arc::new(std::sync::Mutex::new(HashMap::from([(
            "repo-id".to_string(),
            REPOSITORY_RESPONSE.to_string(),
        )])));
        let requests = Arc::new(AtomicUsize::new(0));
        let address = start_upstream(&authority, responses, requests).await;
        let resolver = resolver_for(&authority, address);

        let resolved = resolver
            .resolve("repo-id", NodeKind::Repository, "pat")
            .await
            .unwrap();
        assert_eq!(resolved.owner, "org");
        assert_eq!(resolved.name, "repo");
        assert_eq!(resolved.subject_kind, None);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn subject_node_resolves_to_issue_and_pull_request() {
        let dir = temp_dir("subject");
        let authority = Authority::load_or_create(&dir).expect("authority");
        let responses = Arc::new(std::sync::Mutex::new(HashMap::from([
            ("issue-id".to_string(), ISSUE_RESPONSE.to_string()),
            ("pull-id".to_string(), PULL_REQUEST_RESPONSE.to_string()),
        ])));
        let requests = Arc::new(AtomicUsize::new(0));
        let address = start_upstream(&authority, responses, requests).await;
        let resolver = resolver_for(&authority, address);

        let issue = resolver
            .resolve("issue-id", NodeKind::Subject, "pat")
            .await
            .unwrap();
        assert_eq!(issue.owner, "org");
        assert_eq!(issue.name, "repo");
        assert_eq!(issue.subject_kind, Some(SubjectKind::Issue));

        let pull = resolver
            .resolve("pull-id", NodeKind::Subject, "pat")
            .await
            .unwrap();
        assert_eq!(pull.owner, "acme");
        assert_eq!(pull.name, "widget");
        assert_eq!(pull.subject_kind, Some(SubjectKind::PullRequest));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn repeated_resolve_uses_cache() {
        let dir = temp_dir("cache");
        let authority = Authority::load_or_create(&dir).expect("authority");
        let responses = Arc::new(std::sync::Mutex::new(HashMap::from([(
            "repo-id".to_string(),
            REPOSITORY_RESPONSE.to_string(),
        )])));
        let requests = Arc::new(AtomicUsize::new(0));
        let address = start_upstream(&authority, responses, requests.clone()).await;
        let resolver = resolver_for(&authority, address);

        let first = resolver
            .resolve("repo-id", NodeKind::Repository, "pat")
            .await
            .unwrap();
        let second = resolver
            .resolve("repo-id", NodeKind::Repository, "pat")
            .await
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(requests.load(Ordering::SeqCst), 1);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn unresolved_node_returns_resolution_error() {
        let dir = temp_dir("unresolved");
        let authority = Authority::load_or_create(&dir).expect("authority");
        let responses = Arc::new(std::sync::Mutex::new(HashMap::from([(
            "user-id".to_string(),
            NULL_NODE_RESPONSE.to_string(),
        )])));
        let requests = Arc::new(AtomicUsize::new(0));
        let address = start_upstream(&authority, responses, requests).await;
        let resolver = resolver_for(&authority, address);

        assert!(matches!(
            resolver.resolve("user-id", NodeKind::Repository, "pat").await,
            Err(NodeResolutionError::Resolution)
        ));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn transport_failure_returns_transport_error() {
        let dir = temp_dir("transport");
        let authority = Authority::load_or_create(&dir).expect("authority");
        let resolver = resolver_for(&authority, ephemeral_addr().await);

        assert!(matches!(
            resolver.resolve("repo-id", NodeKind::Repository, "pat").await,
            Err(NodeResolutionError::Transport(_))
        ));

        std::fs::remove_dir_all(dir).unwrap();
    }
}
