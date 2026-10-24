#![deny(missing_docs)]

//! Zoho OAuth and Zoho Mail API client for the Olive Trees fork.
//!
//! Zoho hosts every account in one data center; its OAuth server, Mail API and
//! IMAP server all live under that data center's domain. [`ZohoDataCenter`]
//! names the choice and [`ZohoEndpoints`] resolves it to origins, so tests can
//! point the same client at a mock server.

mod accounts;
mod data_center;
mod error;
mod oauth;
mod signatures;

pub use accounts::{ZohoMailAccount, ZohoMailClient};
pub use data_center::{ZohoDataCenter, ZohoEndpoints, ZohoMailServers, ZohoServerChoice};
pub use error::ZohoError;
pub use oauth::{
    ZohoAccessToken, ZohoAuthorizationRequest, ZohoClientCredentials, ZohoOAuthClient,
    ZohoRefreshToken, ZohoTokenResponse, scopes,
};
pub use signatures::ZohoSignature;

#[cfg(test)]
mod test;
