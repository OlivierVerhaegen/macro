use std::sync::Arc;

use async_imap::{Authenticator, Session};
use tokio::net::TcpStream;
use tokio_rustls::{
    TlsConnector,
    client::TlsStream,
    rustls::{ClientConfig, RootCertStore, pki_types::CertificateDer, pki_types::ServerName},
};
use zeroize::Zeroizing;

use crate::ImapError;

/// An IMAP server reached over implicit TLS (normally port 993).
#[derive(Debug, Clone)]
pub struct ImapServer {
    host: String,
    port: u16,
    roots: Arc<RootCertStore>,
}

impl ImapServer {
    /// A server whose certificate chains to a public web PKI root.
    pub fn new(host: String, port: u16) -> Self {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Self {
            host,
            port,
            roots: Arc::new(roots),
        }
    }

    /// Additionally trust `certificate`, for a self-hosted server with a private CA.
    pub fn with_trusted_certificate(
        mut self,
        certificate: CertificateDer<'static>,
    ) -> Result<Self, ImapError> {
        Arc::make_mut(&mut self.roots)
            .add(certificate)
            .map_err(|error| ImapError::Protocol(format!("untrusted certificate: {error}")))?;
        Ok(self)
    }

    async fn connect_tls(&self) -> Result<TlsStream<TcpStream>, ImapError> {
        let tcp = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(ImapError::Connection)?;
        // Pin the provider: the process may link more than one rustls backend.
        let config = ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|error| ImapError::Protocol(error.to_string()))?
        .with_root_certificates(self.roots.clone())
        .with_no_client_auth();
        let name = ServerName::try_from(self.host.clone())
            .map_err(|_| ImapError::Protocol("invalid IMAP host name".into()))?;
        TlsConnector::from(Arc::new(config))
            .connect(name, tcp)
            .await
            .map_err(ImapError::Connection)
    }
}

/// How to log in. Secrets are zeroized on drop and never printed.
pub enum ImapCredentials {
    /// SASL XOAUTH2 with an OAuth access token (Zoho, Gmail, Outlook).
    XOAuth2 {
        /// The mailbox address the token belongs to.
        username: String,
        /// A current access token.
        access_token: Zeroizing<String>,
    },
    /// Plain `LOGIN` with a password or app password.
    Password {
        /// The login name, usually the address.
        username: String,
        /// The password.
        password: Zeroizing<String>,
    },
}

impl std::fmt::Debug for ImapCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::XOAuth2 { .. } => f.write_str("ImapCredentials::XOAuth2([REDACTED])"),
            Self::Password { .. } => f.write_str("ImapCredentials::Password([REDACTED])"),
        }
    }
}

struct XOAuth2<'a> {
    username: &'a str,
    access_token: &'a str,
}

impl Authenticator for XOAuth2<'_> {
    type Response = Zeroizing<String>;

    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        Zeroizing::new(format!(
            "user={}\x01auth=Bearer {}\x01\x01",
            self.username, self.access_token
        ))
    }
}

/// An authenticated session. One command runs at a time.
pub struct ImapConnection {
    pub(crate) session: Session<TlsStream<TcpStream>>,
    pub(crate) condstore: bool,
    pub(crate) move_supported: bool,
    pub(crate) uidplus: bool,
    pub(crate) idle_supported: bool,
}

impl ImapConnection {
    /// Connect, read the greeting and authenticate.
    #[tracing::instrument(name = "imap.connect", skip_all, fields(port = server.port), err)]
    pub async fn connect(
        server: &ImapServer,
        credentials: &ImapCredentials,
    ) -> Result<Self, ImapError> {
        let mut client = async_imap::Client::new(server.connect_tls().await?);
        client
            .read_response()
            .await
            .map_err(|error| ImapError::Connection(std::io::Error::other(error)))?
            .ok_or_else(|| ImapError::Protocol("missing IMAP greeting".into()))?;
        let session = match credentials {
            ImapCredentials::XOAuth2 {
                username,
                access_token,
            } => client
                .authenticate(
                    "XOAUTH2",
                    XOAuth2 {
                        username,
                        access_token,
                    },
                )
                .await
                .map_err(|_| ImapError::AuthenticationFailed)?,
            ImapCredentials::Password { username, password } => client
                .login(username, password.as_str())
                .await
                .map_err(|_| ImapError::AuthenticationFailed)?,
        };
        let mut session = session;
        // Capabilities can change after authentication; read them now.
        let capabilities = session.capabilities().await?;
        Ok(Self {
            condstore: capabilities.has_str("CONDSTORE"),
            move_supported: capabilities.has_str("MOVE"),
            uidplus: capabilities.has_str("UIDPLUS"),
            idle_supported: capabilities.has_str("IDLE"),
            session,
        })
    }

    /// End the session politely. Failures only mean the server already left.
    pub async fn logout(mut self) {
        let _ = self.session.logout().await;
    }
}
