//! FTP/FTPS storage backend with a local staging layer.
//!
//! A bucket whose storage path assigns an FTP/FTPS connection stores objects in
//! two places at once:
//!
//! * a **local staging directory** (the bucket's `path`), where every object is
//!   written first, and
//! * a **remote FTP/FTPS server**, to which the object is then uploaded.
//!
//! This mirrors the "one path, two locations" model exposed in the admin UI:
//! downloads prefer the local copy (fast, and it survives a server outage) and
//! fall back to the remote copy when the local one is gone. When
//! `delete_local_after_upload` is set, the staged copy is removed after a
//! successful upload so only the remote copy remains.
//!
//! Uploads keep the local copy on failure, so a transient FTP outage never
//! loses data that was already accepted by the API.
//!
//! Keys are opaque, byte-safe identifiers; each operation opens a short-lived
//! connection (connect → optional AUTH TLS → login → transfer → QUIT) rather
//! than pooling connections, which keeps the implementation robust across
//! restarts and idle timeouts.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{AppError, AppResult};
use crate::storage::local::LocalFsBackend;
use crate::utils::traits::StorageBackend;

use suppaftp::tokio::{
    AsyncFtpStream, AsyncRustlsConnector, AsyncRustlsFtpStream, ImplAsyncFtpStream, TokioTlsStream,
};
use suppaftp::types::{FileType, FtpError, Mode};
use suppaftp::Status;

/// Connection settings for the remote FTP/FTPS server.
///
/// Serialised as JSON into a `connections.config` document, so the fields carry
/// serde defaults: a config written by an older build keeps working if a new
/// field is added later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpConfig {
    pub host: String,
    #[serde(default = "default_ftp_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// Remote directory under which objects are stored. Empty = login directory.
    /// Stored relative to the login directory (leading/trailing slashes are
    /// normalised away).
    #[serde(default)]
    pub base_path: String,
    /// Use explicit FTPS (AUTH TLS) instead of plain FTP.
    #[serde(default)]
    pub secure: bool,
    /// Passive-mode data transfers (recommended, and the common default).
    #[serde(default = "default_true")]
    pub passive: bool,
    /// Verify the server certificate (FTPS only). Off accepts self-signed certs.
    #[serde(default = "default_true")]
    pub verify_cert: bool,
}

fn default_ftp_port() -> u16 {
    21
}

fn default_true() -> bool {
    true
}

impl FtpConfig {
    /// Human-readable `host:port` for log/error messages.
    fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// A backend that stages objects locally and mirrors them to FTP/FTPS.
pub struct FtpBackend {
    local: LocalFsBackend,
    config: FtpConfig,
    /// Normalised remote base path (no leading/trailing slash, `..` stripped).
    remote_root: String,
    delete_local_after_upload: bool,
}

impl FtpBackend {
    /// Build a backend rooted at the local staging directory `local_path`.
    ///
    /// This never contacts the FTP server: the staging directory is created on
    /// disk (like a local bucket) and connections are opened lazily per
    /// operation. Existing buckets therefore survive a restart even while the
    /// FTP server is unreachable.
    pub fn new(
        local_path: impl Into<std::path::PathBuf>,
        config: FtpConfig,
        delete_local_after_upload: bool,
    ) -> AppResult<Self> {
        let local = LocalFsBackend::new(local_path)?;
        let remote_root = normalize_remote_base(&config.base_path);
        Ok(Self {
            local,
            config,
            remote_root,
            delete_local_after_upload,
        })
    }

    /// Resolve a storage key to a remote path under the configured base,
    /// rejecting traversal primitives (defense in depth; keys are normally
    /// server-generated content-hash shard paths).
    fn remote_path(&self, key: &str) -> AppResult<String> {
        if key.is_empty() {
            return Err(AppError::BadRequest("empty storage key not allowed".into()));
        }
        if key.contains('\0') {
            return Err(AppError::BadRequest("invalid storage key".into()));
        }

        let mut parts = Vec::new();
        for seg in key.split('/') {
            match seg {
                "" | "." => continue,
                ".." => {
                    return Err(AppError::BadRequest("path traversal not allowed".into()));
                }
                s => parts.push(s),
            }
        }
        if parts.is_empty() {
            return Err(AppError::BadRequest(
                "storage key resolves to the backend root".into(),
            ));
        }

        let rel = parts.join("/");
        if self.remote_root.is_empty() {
            Ok(rel)
        } else {
            Ok(format!("{}/{}", self.remote_root, rel))
        }
    }

    /// Open a fresh connection and log in, ready for a transfer.
    async fn open(&self) -> AppResult<FtpConn> {
        connect_and_login(&self.config).await
    }
}

/// Open a fresh connection and log in, ready for a transfer. Shared by every
/// backend operation and by the admin "check connection" probe.
async fn connect_and_login(cfg: &FtpConfig) -> AppResult<FtpConn> {
    if cfg.secure {
        let stream = AsyncRustlsFtpStream::connect((cfg.host.as_str(), cfg.port))
            .await
            .map_err(|e| AppError::Storage(format!("FTPS connect to {} failed: {e}", cfg.addr())))?;
        let connector = build_rustls_connector(cfg.verify_cert)?;
        let mut stream = stream.into_secure(connector, &cfg.host).await.map_err(|e| {
            AppError::Storage(format!("FTPS handshake with {} failed: {e}", cfg.addr()))
        })?;
        configure_session(&mut stream, cfg).await?;
        Ok(FtpConn::Secure(stream))
    } else {
        let mut stream = AsyncFtpStream::connect((cfg.host.as_str(), cfg.port))
            .await
            .map_err(|e| AppError::Storage(format!("FTP connect to {} failed: {e}", cfg.addr())))?;
        configure_session(&mut stream, cfg).await?;
        Ok(FtpConn::Plain(stream))
    }
}

/// Verify that `config` can reach and authenticate against the server: connect,
/// log in, then disconnect. Used by the admin UI to check a saved or
/// not-yet-saved connection; it never transfers data.
pub async fn check_connection(config: &FtpConfig) -> AppResult<()> {
    if config.host.trim().is_empty() {
        return Err(AppError::Validation("FTP host is required".into()));
    }
    let mut conn = connect_and_login(config).await?;
    conn.quit().await;
    Ok(())
}

/// An authenticated connection; `Plain` for FTP, `Secure` for FTPS. Both wrap
/// the same generic `ImplAsyncFtpStream<T>`, so each operation dispatches to a
/// single generic implementation.
enum FtpConn {
    Plain(AsyncFtpStream),
    Secure(AsyncRustlsFtpStream),
}

impl FtpConn {
    async fn upload(&mut self, remote: &str, data: &[u8]) -> AppResult<()> {
        match self {
            FtpConn::Plain(s) => ftp_upload(s, remote, data).await,
            FtpConn::Secure(s) => ftp_upload(s, remote, data).await,
        }
    }

    async fn download(&mut self, remote: &str) -> AppResult<Option<Bytes>> {
        match self {
            FtpConn::Plain(s) => ftp_download(s, remote).await,
            FtpConn::Secure(s) => ftp_download(s, remote).await,
        }
    }

    async fn remove(&mut self, remote: &str) -> AppResult<bool> {
        match self {
            FtpConn::Plain(s) => ftp_remove(s, remote).await,
            FtpConn::Secure(s) => ftp_remove(s, remote).await,
        }
    }

    async fn exists(&mut self, remote: &str) -> AppResult<bool> {
        match self {
            FtpConn::Plain(s) => ftp_exists(s, remote).await,
            FtpConn::Secure(s) => ftp_exists(s, remote).await,
        }
    }

    /// Best-effort QUIT; failures are ignored because the operation already
    /// completed (or already failed) by the time we get here.
    async fn quit(&mut self) {
        let _ = match self {
            FtpConn::Plain(s) => s.quit().await,
            FtpConn::Secure(s) => s.quit().await,
        };
    }
}

// ── generic session helpers (shared by FTP and FTPS) ────────────────────────

/// Log in and switch to binary/passive (or active) transfers.
async fn configure_session<T>(stream: &mut ImplAsyncFtpStream<T>, cfg: &FtpConfig) -> AppResult<()>
where
    T: TokioTlsStream + Send,
{
    stream
        .login(&cfg.username, &cfg.password)
        .await
        .map_err(|e| AppError::Storage(format!("FTP login to {} failed: {e}", cfg.addr())))?;
    stream.set_mode(if cfg.passive {
        Mode::Passive
    } else {
        Mode::Active
    });
    stream
        .transfer_type(FileType::Binary)
        .await
        .map_err(|e| AppError::Storage(format!("FTP TYPE I failed: {e}")))?;
    Ok(())
}

async fn ftp_upload<T>(
    stream: &mut ImplAsyncFtpStream<T>,
    remote: &str,
    data: &[u8],
) -> AppResult<()>
where
    T: TokioTlsStream + Send,
{
    ensure_remote_dirs(stream, remote).await;
    let mut data_stream = stream
        .put_with_stream(remote)
        .await
        .map_err(|e| AppError::Storage(format!("FTP STOR '{remote}' failed: {e}")))?;
    data_stream
        .write_all(data)
        .await
        .map_err(|e| AppError::Storage(format!("FTP upload of '{remote}' failed: {e}")))?;
    data_stream
        .finish()
        .await
        .map_err(|e| AppError::Storage(format!("FTP upload of '{remote}' failed: {e}")))?;
    Ok(())
}

async fn ftp_download<T>(
    stream: &mut ImplAsyncFtpStream<T>,
    remote: &str,
) -> AppResult<Option<Bytes>>
where
    T: TokioTlsStream + Send,
{
    let mut data_stream = match stream.retr_as_stream(remote).await {
        Ok(s) => s,
        Err(e) if is_not_found(&e) => return Ok(None),
        Err(e) => {
            return Err(AppError::Storage(format!(
                "FTP RETR '{remote}' failed: {e}"
            )));
        }
    };
    let mut buf = Vec::new();
    data_stream
        .read_to_end(&mut buf)
        .await
        .map_err(|e| AppError::Storage(format!("FTP download of '{remote}' failed: {e}")))?;
    data_stream
        .finish()
        .await
        .map_err(|e| AppError::Storage(format!("FTP download of '{remote}' failed: {e}")))?;
    Ok(Some(Bytes::from(buf)))
}

async fn ftp_remove<T>(stream: &mut ImplAsyncFtpStream<T>, remote: &str) -> AppResult<bool>
where
    T: TokioTlsStream + Send,
{
    match stream.rm(remote).await {
        Ok(()) => Ok(true),
        Err(e) if is_not_found(&e) => Ok(false),
        Err(e) => Err(AppError::Storage(format!("FTP DELE '{remote}' failed: {e}"))),
    }
}

async fn ftp_exists<T>(stream: &mut ImplAsyncFtpStream<T>, remote: &str) -> AppResult<bool>
where
    T: TokioTlsStream + Send,
{
    // SIZE is the cheapest existence probe (RFC 3659). If the server does not
    // implement it, fall back to MLST.
    match stream.size(remote).await {
        Ok(_) => Ok(true),
        Err(e) if is_not_found(&e) => Ok(false),
        Err(_) => match stream.mlst(Some(remote)).await {
            Ok(_) => Ok(true),
            Err(e) if is_not_found(&e) => Ok(false),
            Err(e) => Err(AppError::Storage(format!(
                "FTP probe of '{remote}' failed: {e}"
            ))),
        },
    }
}

/// Create every parent directory of `remote`, ignoring "already exists" errors.
async fn ensure_remote_dirs<T>(stream: &mut ImplAsyncFtpStream<T>, remote: &str)
where
    T: TokioTlsStream + Send,
{
    let Some(dir) = parent_dir(remote) else {
        return;
    };
    let mut path = String::new();
    if dir.starts_with('/') {
        path.push('/');
    }
    for seg in dir.split('/').filter(|s| !s.is_empty()) {
        if !path.is_empty() && !path.ends_with('/') {
            path.push('/');
        }
        path.push_str(seg);
        // Ignore failures: an existing directory is the common case, and a real
        // problem resurfaces as a STOR error with a clearer message.
        let _ = stream.mkdir(&path).await;
    }
}

/// Directory portion of a remote path, or `None` when there is none.
fn parent_dir(path: &str) -> Option<&str> {
    match path.rfind('/') {
        Some(0) | None => None,
        Some(i) => Some(&path[..i]),
    }
}

/// Whether an error means "the requested path does not exist".
fn is_not_found(e: &FtpError) -> bool {
    matches!(e, FtpError::UnexpectedResponse(r) if r.status == Status::FileUnavailable)
}

/// Normalise a user-supplied remote base path: strip surrounding slashes, drop
/// `.`/empty segments and any `..` components.
fn normalize_remote_base(raw: &str) -> String {
    raw.trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .collect::<Vec<_>>()
        .join("/")
}

#[async_trait]
impl StorageBackend for FtpBackend {
    async fn put(&self, key: &str, body: Bytes) -> AppResult<()> {
        let remote = self.remote_path(key)?;

        // 1. Always stage locally first: this is the write that never loses data.
        self.local.put(key, body.clone()).await?;

        // 2. Mirror to the FTP server. On failure the staged copy is kept so the
        //    object is retryable and still downloadable.
        let mut conn = self.open().await?;
        let upload = conn.upload(&remote, &body).await;
        conn.quit().await;
        upload?;

        // 3. Optionally drop the local copy once it is safely on the server.
        if self.delete_local_after_upload {
            self.local.delete(key).await?;
        }
        Ok(())
    }

    async fn get(&self, key: &str) -> AppResult<Option<Bytes>> {
        // Prefer the local staged copy.
        if let Some(data) = self.local.get(key).await? {
            return Ok(Some(data));
        }

        let remote = self.remote_path(key)?;
        let mut conn = self.open().await?;
        let data = conn.download(&remote).await;
        conn.quit().await;
        let data = data?;

        // Refresh the local cache when we keep staged copies, so subsequent
        // reads stay local.
        if let Some(ref bytes) = data {
            if !self.delete_local_after_upload {
                let _ = self.local.put(key, bytes.clone()).await;
            }
        }
        Ok(data)
    }

    async fn delete(&self, key: &str) -> AppResult<bool> {
        let local_deleted = self.local.delete(key).await.unwrap_or(false);

        let remote = self.remote_path(key)?;
        let mut conn = self.open().await?;
        let remote_deleted = conn.remove(&remote).await;
        conn.quit().await;
        let remote_deleted = remote_deleted?;

        Ok(local_deleted || remote_deleted)
    }

    async fn exists(&self, key: &str) -> AppResult<bool> {
        if self.local.exists(key).await? {
            return Ok(true);
        }
        let remote = self.remote_path(key)?;
        let mut conn = self.open().await?;
        let found = conn.exists(&remote).await;
        conn.quit().await;
        found
    }

    /// Best-effort listing of staged local keys under `prefix`.
    ///
    /// The application never calls `list` on bucket backends (only `get` and
    /// `delete`), so this intentionally reports the local staging view rather
    /// than issuing (and parsing) a recursive remote `MLSD`. With
    /// `delete_local_after_upload` enabled the staging directory is empty and
    /// this returns no keys.
    async fn list(&self, prefix: &str) -> AppResult<Vec<String>> {
        self.local.list(prefix).await
    }

    fn scheme(&self) -> &str {
        "ftp"
    }
}

// ── TLS plumbing ────────────────────────────────────────────────────────────

/// Build a rustls connector for FTPS. When `verify` is false the certificate
/// chain is accepted as-is (self-signed FTPS servers); handshake signatures are
/// still verified.
fn build_rustls_connector(verify: bool) -> AppResult<AsyncRustlsConnector> {
    use suppaftp::rustls;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| AppError::Internal(format!("failed to initialise TLS: {e}")))?;

    let client_config = if verify {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertVerifier::new(&provider)))
            .with_no_client_auth()
    };

    let connector = suppaftp::tokio_rustls::TlsConnector::from(Arc::new(client_config));
    Ok(AsyncRustlsConnector::from(connector))
}

/// A `ServerCertVerifier` that accepts any certificate. Used only when the
/// admin explicitly disables certificate verification for an FTPS bucket.
#[derive(Debug)]
struct NoCertVerifier {
    algorithms: suppaftp::rustls::crypto::WebPkiSupportedAlgorithms,
}

impl NoCertVerifier {
    fn new(provider: &suppaftp::rustls::crypto::CryptoProvider) -> Self {
        Self {
            algorithms: provider.signature_verification_algorithms,
        }
    }
}

impl suppaftp::rustls::client::danger::ServerCertVerifier for NoCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &suppaftp::rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[suppaftp::rustls::pki_types::CertificateDer<'_>],
        _server_name: &suppaftp::rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: suppaftp::rustls::pki_types::UnixTime,
    ) -> Result<suppaftp::rustls::client::danger::ServerCertVerified, suppaftp::rustls::Error> {
        Ok(suppaftp::rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &suppaftp::rustls::pki_types::CertificateDer<'_>,
        dss: &suppaftp::rustls::DigitallySignedStruct,
    ) -> Result<suppaftp::rustls::client::danger::HandshakeSignatureValid, suppaftp::rustls::Error>
    {
        suppaftp::rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &suppaftp::rustls::pki_types::CertificateDer<'_>,
        dss: &suppaftp::rustls::DigitallySignedStruct,
    ) -> Result<suppaftp::rustls::client::danger::HandshakeSignatureValid, suppaftp::rustls::Error>
    {
        suppaftp::rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<suppaftp::rustls::SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_base_strips_and_drops_traversal() {
        assert_eq!(normalize_remote_base(""), "");
        assert_eq!(normalize_remote_base("/"), "");
        assert_eq!(normalize_remote_base("/uploads/"), "uploads");
        assert_eq!(normalize_remote_base("a//b/./c"), "a/b/c");
        assert_eq!(normalize_remote_base("../../etc"), "etc");
    }

    #[test]
    fn parent_dir_extracts_directory() {
        assert_eq!(parent_dir("abc"), None);
        assert_eq!(parent_dir("a/b/c"), Some("a/b"));
        assert_eq!(parent_dir("/a/b"), Some("/a"));
        assert_eq!(parent_dir("/a"), None);
    }

    #[test]
    fn remote_path_joins_base_and_rejects_traversal() {
        let backend = FtpBackend::new(
            std::env::temp_dir().join("keystone-ftp-test-path"),
            FtpConfig {
                host: "localhost".into(),
                port: 21,
                username: "u".into(),
                password: "p".into(),
                base_path: "/data".into(),
                secure: false,
                passive: true,
                verify_cert: true,
            },
            false,
        )
        .unwrap();

        assert_eq!(backend.remote_path("ab/cd").unwrap(), "data/ab/cd");
        assert!(backend.remote_path("").is_err());
        assert!(backend.remote_path("../escape").is_err());
        assert!(backend.remote_path("a/../../b").is_err());
    }
}
