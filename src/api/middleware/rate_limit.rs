//! Per-IP sliding-window rate limiting middleware.
//!
//! Implemented with `std::time` only (no timers, no new dependencies). The
//! client address comes from `ConnectInfo<SocketAddr>` — the TCP peer address —
//! which is populated when the app is served with
//! `axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())`.
//!
//! By default the `X-Forwarded-For` header is ignored so a client can never
//! spoof its identity. When the server runs behind a reverse proxy the TCP peer
//! is the proxy, so all users would share one bucket; enabling
//! `trust_proxy_headers` derives the client IP from the leftmost
//! `X-Forwarded-For` entry instead. Operators MUST ensure their proxy
//! overwrites or strips the header before it reaches the app, or clients can
//! rotate it to evade the limit.
//!
//! The limiter is stateful and must be wrapped in `Arc` and injected with
//! `axum::middleware::from_fn_with_state` (see `main.rs`).

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tokio::sync::RwLock;

/// Sliding-window per-IP rate limiter.
///
/// For each client IP the timestamps of recent requests are kept in a deque.
/// A request is admitted while fewer than `burst_size` timestamps fall within
/// the trailing window, and the window length is `burst_size /
/// requests_per_second` seconds (minimum 1s), so the sustained rate equals
/// `requests_per_second` while still allowing a burst of `burst_size`.
pub struct RateLimiter {
    enabled: bool,
    capacity: usize,
    window: Duration,
    windows: RwLock<HashMap<IpAddr, VecDeque<Instant>>>,
    /// Hard cap on the number of tracked IPs so memory cannot grow without
    /// bound (e.g. under a distributed/spoofed-source attack).
    max_tracked_ips: usize,
    /// Trust the leftmost `X-Forwarded-For` entry as the client IP (required
    /// behind a reverse proxy). See the module docs for the trust caveat.
    trust_proxy_headers: bool,
}

impl RateLimiter {
    pub fn new(
        enabled: bool,
        requests_per_second: u64,
        burst_size: u32,
        trust_proxy_headers: bool,
    ) -> Self {
        let rps = requests_per_second.max(1);
        // Window length: ceil(burst_size / requests_per_second), at least 1s.
        // A burst of `burst_size` requests may pass at once; the sustained
        // rate is `requests_per_second`.
        let window_secs = (burst_size.max(1) as u64).div_ceil(rps).max(1);
        Self {
            enabled,
            capacity: burst_size.max(1) as usize,
            window: Duration::from_secs(window_secs),
            windows: RwLock::new(HashMap::new()),
            max_tracked_ips: 100_000,
            trust_proxy_headers,
        }
    }

    /// Returns `true` if the request from `ip` should be admitted.
    pub async fn check(&self, ip: IpAddr) -> bool {
        if !self.enabled {
            return true;
        }

        let now = Instant::now();
        let mut windows = self.windows.write().await;

        let deque = windows.entry(ip).or_default();

        // Drop timestamps that have fallen out of the window (front is the
        // oldest because timestamps are pushed in order).
        while deque.front().is_some_and(|t| now.duration_since(*t) > self.window) {
            deque.pop_front();
        }

        if deque.len() >= self.capacity {
            return false;
        }

        deque.push_back(now);

        // Bound memory: when the tracked-IP cap is exceeded, evict an entry
        // other than the current IP. Only new IPs trigger eviction, so repeat
        // clients are never churned out.
        if windows.len() > self.max_tracked_ips {
            let victim = windows.keys().copied().find(|k| k != &ip);
            if let Some(victim) = victim {
                windows.remove(&victim);
            }
        }

        true
    }
}

/// Resolve the IP to rate-limit. When proxy headers are trusted, the leftmost
/// `X-Forwarded-For` entry is used (the original client); otherwise the TCP
/// peer address. Returns `None` only when no address can be determined at all.
fn client_ip(
    limiter: &RateLimiter,
    connect_info: &Option<ConnectInfo<SocketAddr>>,
    headers: &axum::http::HeaderMap,
) -> Option<IpAddr> {
    if limiter.trust_proxy_headers {
        if let Some(xff) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
        {
            if let Some(first) = xff.split(',').next().map(str::trim).filter(|s| !s.is_empty()) {
                if let Ok(ip) = first.parse::<IpAddr>() {
                    return Some(ip);
                }
            }
        }
    }
    connect_info.as_ref().map(|ci| ci.0.ip())
}

/// Axum middleware applying the rate limiter. The limiter state is injected
/// via `axum::middleware::from_fn_with_state`.
pub async fn rate_limit(
    State(limiter): State<Arc<RateLimiter>>,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    headers: axum::http::HeaderMap,
    req: Request<Body>,
    next: Next,
) -> Response {
    let admitted = match client_ip(&limiter, &connect_info, &headers) {
        // If no client address is determinable (e.g. callers that bypass
        // `into_make_service_with_connect_info` and no trusted header), do not
        // rate limit rather than blocking legitimate traffic.
        Some(ip) => limiter.check(ip).await,
        None => true,
    };

    if !admitted {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "1")],
            axum::Json(serde_json::json!({
                "type": "/errors/RATE_LIMITED",
                "title": "Too Many Requests",
                "status": 429,
                "detail": "rate limit exceeded, try again later",
            })),
        )
            .into_response();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_disabled_always_allows() {
        let limiter = RateLimiter::new(false, 1, 1, false);
        for _ in 0..5 {
            assert!(limiter.check(IpAddr::from([127, 0, 0, 1])).await);
        }
    }

    #[tokio::test]
    async fn test_burst_then_limited() {
        let limiter = RateLimiter::new(true, 1000, 3, false);
        let ip = IpAddr::from([127, 0, 0, 1]);

        for _ in 0..3 {
            assert!(limiter.check(ip).await, "first burst_size requests allowed");
        }
        assert!(!limiter.check(ip).await, "burst capacity exhausted");

        // A different IP is unaffected.
        assert!(limiter.check(IpAddr::from([127, 0, 0, 2])).await);
    }

    #[tokio::test]
    async fn test_window_recovers() {
        // rps=1000, burst=1 -> window is 1s.
        let limiter = RateLimiter::new(true, 1000, 1, false);
        let ip = IpAddr::from([127, 0, 0, 1]);

        assert!(limiter.check(ip).await);
        assert!(!limiter.check(ip).await);

        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(limiter.check(ip).await, "window elapsed, request admitted");
    }

    fn header_map(entries: &[(&str, &str)]) -> axum::http::HeaderMap {
        entries
            .iter()
            .map(|(k, v)| {
                (
                    axum::http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                    v.parse().unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn test_client_ip_uses_peer_by_default() {
        let limiter = RateLimiter::new(true, 1, 1, false);
        let connect = Some(ConnectInfo(SocketAddr::from(([10, 0, 0, 1], 4000))));
        let headers = header_map(&[("x-forwarded-for", "203.0.113.9")]);
        assert_eq!(
            client_ip(&limiter, &connect, &headers),
            Some(IpAddr::from([10, 0, 0, 1]))
        );
    }

    #[test]
    fn test_client_ip_uses_xff_when_trusted() {
        let limiter = RateLimiter::new(true, 1, 1, true);
        let connect = Some(ConnectInfo(SocketAddr::from(([10, 0, 0, 1], 4000))));
        let headers = header_map(&[("x-forwarded-for", "203.0.113.9, 10.0.0.1")]);
        assert_eq!(
            client_ip(&limiter, &connect, &headers),
            Some(IpAddr::from([203, 0, 113, 9]))
        );
    }

    #[test]
    fn test_client_ip_falls_back_when_xff_garbage() {
        let limiter = RateLimiter::new(true, 1, 1, true);
        let connect = Some(ConnectInfo(SocketAddr::from(([10, 0, 0, 1], 4000))));
        let headers = header_map(&[("x-forwarded-for", "not-an-ip")]);
        assert_eq!(
            client_ip(&limiter, &connect, &headers),
            Some(IpAddr::from([10, 0, 0, 1]))
        );
    }
}
