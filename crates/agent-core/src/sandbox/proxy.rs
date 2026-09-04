//! Host-side egress proxy for domain filtering.
//!
//! A CONNECT proxy that inspects the destination hostname without terminating TLS,
//! allowing fine-grained egress narrowing ("only crates.io") when `network = true`.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

/// Matches destination hostnames against an allowlist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EgressFilter {
    /// Permitted domain patterns. An empty list permits all destinations.
    pub allowlist: Vec<String>,
}

impl EgressFilter {
    pub fn new(allowlist: Vec<String>) -> Self {
        Self {
            allowlist: allowlist.into_iter().map(|s| s.to_lowercase()).collect(),
        }
    }

    /// Whether destination `host` matches any pattern in the allowlist.
    pub fn is_allowed(&self, host: &str) -> bool {
        if self.allowlist.is_empty() {
            return true;
        }
        let host = host.split(':').next().unwrap_or(host).to_lowercase();
        for pattern in &self.allowlist {
            if let Some(suffix) = pattern.strip_prefix("*.") {
                if host == suffix || host.ends_with(&pattern[1..]) {
                    return true;
                }
            } else if &host == pattern {
                return true;
            }
        }
        false
    }
}

/// A lightweight HTTP CONNECT proxy.
pub struct EgressProxy {
    addr: SocketAddr,
    filter: Arc<RwLock<EgressFilter>>,
    shutdown: tokio::sync::broadcast::Sender<()>,
}

impl EgressProxy {
    /// Binds an egress proxy on the given local address with an initial filter.
    pub async fn bind(addr: SocketAddr, allowlist: Vec<String>) -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        let filter = Arc::new(RwLock::new(EgressFilter::new(allowlist)));
        let (shutdown, _) = tokio::sync::broadcast::channel(1);

        let filter_clone = filter.clone();
        let mut shutdown_rx = shutdown.subscribe();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept_res = listener.accept() => {
                        let Ok((stream, _)) = accept_res else {
                            break;
                        };
                        let filter = filter_clone.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, filter).await {
                                // Debug trace or silent ignore of dropped clients
                                let _ = e;
                            }
                        });
                    }
                    _ = shutdown_rx.recv() => {
                        break;
                    }
                }
            }
        });

        Ok(Self {
            addr: local_addr,
            filter,
            shutdown,
        })
    }

    /// The local socket address the proxy is listening on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Formats the proxy URL for `HTTP_PROXY` / `HTTPS_PROXY`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Updates the allowlist for the running proxy.
    pub async fn set_allowlist(&self, allowlist: Vec<String>) {
        let mut filter = self.filter.write().await;
        *filter = EgressFilter::new(allowlist);
    }
}

impl Drop for EgressProxy {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
    }
}

async fn handle_connection(
    mut client: TcpStream,
    filter: Arc<RwLock<EgressFilter>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut reader = BufReader::new(&mut client);
    let mut line = String::new();
    if reader.read_line(&mut line).await? == 0 {
        return Ok(());
    }

    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }

    let method = parts[0];
    let target = parts[1];

    if method.eq_ignore_ascii_case("CONNECT") {
        // Target is host:port
        let host = target.split(':').next().unwrap_or(target);
        let port = target
            .split(':')
            .nth(1)
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(443);

        let allowed = filter.read().await.is_allowed(host);
        if !allowed {
            let response = format!(
                "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\ndestination `{host}` is not allowed by the egress policy\n"
            );
            client.write_all(response.as_bytes()).await?;
            client.shutdown().await?;
            return Ok(());
        }

        // Connect to upstream destination
        match TcpStream::connect((host, port)).await {
            Ok(mut upstream) => {
                client
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await?;
                let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
            }
            Err(_) => {
                client
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
                    .await?;
                client.shutdown().await?;
            }
        }
    } else {
        // Plain HTTP request (e.g. GET http://host:port/path)
        let host = if target.starts_with("http://") {
            target
                .strip_prefix("http://")
                .unwrap_or(target)
                .split('/')
                .next()
                .unwrap_or("")
        } else {
            ""
        };
        let host_clean = host.split(':').next().unwrap_or(host);
        let port = host
            .split(':')
            .nth(1)
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(80);

        let allowed = filter.read().await.is_allowed(host_clean);
        if !allowed {
            let response = format!(
                "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\ndestination `{host_clean}` is not allowed by the egress policy\n"
            );
            client.write_all(response.as_bytes()).await?;
            client.shutdown().await?;
            return Ok(());
        }

        match TcpStream::connect((host_clean, port)).await {
            Ok(mut upstream) => {
                // Forward the first line and tunnel the rest
                upstream.write_all(line.as_bytes()).await?;
                let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
            }
            Err(_) => {
                client
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
                    .await?;
                client.shutdown().await?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_matching() {
        let filter = EgressFilter::new(vec![
            "crates.io".into(),
            "*.github.com".into(),
            "127.0.0.1".into(),
        ]);

        assert!(filter.is_allowed("crates.io"));
        assert!(filter.is_allowed("crates.io:443"));
        assert!(filter.is_allowed("api.github.com"));
        assert!(filter.is_allowed("raw.github.com:443"));
        assert!(filter.is_allowed("github.com"));
        assert!(filter.is_allowed("127.0.0.1:8080"));

        assert!(!filter.is_allowed("evil.com"));
        assert!(!filter.is_allowed("notgithub.com"));
        assert!(!filter.is_allowed("crates.io.attacker.com"));
    }

    #[test]
    fn empty_filter_allows_all() {
        let filter = EgressFilter::default();
        assert!(filter.is_allowed("crates.io"));
        assert!(filter.is_allowed("anything.org"));
    }

    #[tokio::test]
    async fn proxy_connect_denial_and_grant() {
        // Echo / dummy upstream server
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = upstream.accept().await {
                let mut buf = [0u8; 5];
                let _ = tokio::io::AsyncReadExt::read_exact(&mut stream, &mut buf).await;
                let _ = stream.write_all(b"pong\n").await;
            }
        });

        let proxy = EgressProxy::bind("127.0.0.1:0".parse().unwrap(), vec!["127.0.0.1".into()])
            .await
            .unwrap();

        // 1. Allowed connection
        let mut client = TcpStream::connect(proxy.addr()).await.unwrap();
        let connect_req = format!(
            "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            upstream_addr.port(),
            upstream_addr.port()
        );
        client.write_all(connect_req.as_bytes()).await.unwrap();

        let mut reader = BufReader::new(&mut client);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.contains("200 Connection Established"));

        // Send payload and receive echo
        reader.get_mut().write_all(b"hello").await.unwrap();
        let mut resp = String::new();
        // Skip empty line from 200
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        reader.read_line(&mut resp).await.unwrap();
        assert_eq!(resp, "pong\n");

        // 2. Denied connection
        let mut client_denied = TcpStream::connect(proxy.addr()).await.unwrap();
        client_denied
            .write_all(b"CONNECT forbidden.com:443 HTTP/1.1\r\nHost: forbidden.com:443\r\n\r\n")
            .await
            .unwrap();

        let mut reader_denied = BufReader::new(&mut client_denied);
        let mut line_denied = String::new();
        reader_denied.read_line(&mut line_denied).await.unwrap();
        assert!(line_denied.contains("403 Forbidden"));
    }
}
