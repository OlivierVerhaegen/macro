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

    /// IMAP over implicit TLS, port 993.
    pub fn imap_host(self) -> String {
        format!("imap.{}", self.domain())
    }

    /// SMTP over implicit TLS, port 465.
    pub fn smtp_host(self) -> String {
        format!("smtp.{}", self.domain())
    }
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
