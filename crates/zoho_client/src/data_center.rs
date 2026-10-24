use url::Url;

/// A Zoho data center. An account exists in exactly one, chosen at signup.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    strum::EnumString,
    strum::Display,
    strum::AsRefStr,
    strum::EnumIter,
)]
pub enum ZohoDataCenter {
    /// United States, `zoho.com`.
    #[serde(rename = "com")]
    #[strum(serialize = "com")]
    Com,
    /// European Union, `zoho.eu`.
    #[serde(rename = "eu")]
    #[strum(serialize = "eu")]
    Eu,
    /// India, `zoho.in`.
    #[serde(rename = "in")]
    #[strum(serialize = "in")]
    In,
    /// Australia, `zoho.com.au`.
    #[serde(rename = "com.au")]
    #[strum(serialize = "com.au")]
    ComAu,
    /// Japan, `zoho.jp`.
    #[serde(rename = "jp")]
    #[strum(serialize = "jp")]
    Jp,
    /// Canada, `zohocloud.ca`.
    #[serde(rename = "ca")]
    #[strum(serialize = "ca")]
    Ca,
    /// Saudi Arabia, `zoho.sa`.
    #[serde(rename = "sa")]
    #[strum(serialize = "sa")]
    Sa,
    /// China, `zoho.com.cn`.
    #[serde(rename = "com.cn")]
    #[strum(serialize = "com.cn")]
    ComCn,
}

impl ZohoDataCenter {
    /// The registrable domain all of this data center's services live under.
    pub fn domain(self) -> &'static str {
        match self {
            Self::Com => "zoho.com",
            Self::Eu => "zoho.eu",
            Self::In => "zoho.in",
            Self::ComAu => "zoho.com.au",
            Self::Jp => "zoho.jp",
            Self::Ca => "zohocloud.ca",
            Self::Sa => "zoho.sa",
            Self::ComCn => "zoho.com.cn",
        }
    }

    /// The IMAP (implicit TLS, port 993) and SMTP (implicit TLS, port 465)
    /// servers for a mailbox address. Zoho serves paid organizations on their
    /// own domain from `imappro`/`smtppro`, and personal `@zoho`/`@zohomail`
    /// addresses and free organizations from `imap`/`smtp`; a client on the
    /// wrong pair stops working. The address cannot tell a free organization
    /// apart, so [`ZohoServerChoice::Auto`] assumes a domain address is paid.
    pub fn mail_servers(self, mailbox_address: &str, choice: ZohoServerChoice) -> ZohoMailServers {
        let pro = match choice {
            ZohoServerChoice::Pro => true,
            ZohoServerChoice::Standard => false,
            ZohoServerChoice::Auto => !mailbox_address
                .rsplit_once('@')
                .map_or("", |(_, domain)| domain)
                .to_ascii_lowercase()
                .split_once('.')
                .is_some_and(|(name, _)| name == "zoho" || name == "zohomail"),
        };
        let suffix = if pro { "pro" } else { "" };
        ZohoMailServers {
            imap_host: format!("imap{suffix}.{}", self.domain()),
            smtp_host: format!("smtp{suffix}.{}", self.domain()),
        }
    }
}

/// Which Zoho mail servers a connection uses; see [`ZohoDataCenter::mail_servers`].
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    strum::EnumString,
    strum::Display,
    strum::AsRefStr,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ZohoServerChoice {
    /// `imappro`/`smtppro` for a domain address, else `imap`/`smtp`.
    #[default]
    Auto,
    /// `imappro`/`smtppro`: paid organizations on their own domain.
    Pro,
    /// `imap`/`smtp`: personal accounts and free organizations.
    Standard,
}

/// The IMAP and SMTP hosts of one Zoho mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZohoMailServers {
    /// IMAP over implicit TLS, port 993.
    pub imap_host: String,
    /// SMTP over implicit TLS, port 465.
    pub smtp_host: String,
}

/// Origins for one data center's OAuth server and Mail API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZohoEndpoints {
    /// The OAuth server, e.g. `https://accounts.zoho.eu`.
    pub accounts: Url,
    /// The Mail REST API, e.g. `https://mail.zoho.eu`.
    pub mail_api: Url,
}

impl ZohoEndpoints {
    /// The production endpoints of a data center.
    pub fn for_data_center(data_center: ZohoDataCenter) -> Self {
        let origin = |service: &str| {
            Url::parse(&format!("https://{service}.{}", data_center.domain()))
                .expect("every data center domain forms a valid https origin")
        };
        Self {
            accounts: origin("accounts"),
            mail_api: origin("mail"),
        }
    }
}
