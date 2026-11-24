//! PSN account ID, needed to register with a console.
//!
//! Same sign-in flow as chiaki-ng: the user signs in on Sony's page in a
//! browser, gets redirected to a URL carrying a one-time `code`, and we trade
//! that code for an access token and read the numeric account ID with it.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::error::{Error, Result};

// The Remote Play app's public OAuth client, as used by chiaki-ng.
const CLIENT_ID: &str = "ba495a24-818c-472b-b12d-ff231c1b5745";
const CLIENT_SECRET: &str = "mvaiZkRsAsI1IBkY";
const AUTHORIZE_URL: &str = "https://auth.api.sonyentertainmentnetwork.com/2.0/oauth/authorize";
const TOKEN_URL: &str = "https://auth.api.sonyentertainmentnetwork.com/2.0/oauth/token";
const REDIRECT_URI: &str = "https://remoteplay.dl.playstation.net/remoteplay/redirect";
const SCOPE: &str = "psn:clientapp referenceDataService:countryConfig.read \
    pushNotification:webSocket.desktop.connect sessionManager:remotePlaySession.system.update";

/// The 8-byte PSN account ID, shown and stored as base64 (like chiaki-ng).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AccountId([u8; 8]);

impl AccountId {
    pub fn from_bytes(bytes: [u8; 8]) -> Self {
        Self(bytes)
    }

    /// From the numeric `user_id` the PSN API returns (little-endian bytes).
    pub fn from_user_id(user_id: u64) -> Self {
        Self(user_id.to_le_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; 8] {
        &self.0
    }

    pub fn to_base64(&self) -> String {
        BASE64.encode(self.0)
    }
}

impl FromStr for AccountId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let bytes = BASE64
            .decode(s.trim())
            .map_err(|_| Error::InvalidAccountId)?;
        let bytes: [u8; 8] = bytes.try_into().map_err(|_| Error::InvalidAccountId)?;
        Ok(Self(bytes))
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_base64())
    }
}

impl fmt::Debug for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AccountId({})", self.to_base64())
    }
}

/// The page to open in a browser to sign in.
pub fn login_url() -> String {
    let mut url = url::Url::parse(AUTHORIZE_URL).unwrap();
    url.query_pairs_mut()
        .append_pair("service_entity", "urn:service-entity:psn")
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPE)
        .append_pair("request_locale", "en_US")
        .append_pair("ui", "pr")
        .append_pair("service_logo", "ps")
        .append_pair("layout_type", "popup")
        .append_pair("smcid", "remoteplay")
        .append_pair("prompt", "always")
        .append_pair("PlatformPrivacyWs1", "minimal");
    url.into()
}

/// Pulls the one-time `code` out of the URL the browser lands on after
/// signing in. A bare code is accepted too.
pub fn parse_redirect(input: &str) -> Result<String> {
    let input = input.trim();
    if !input.contains("://") {
        return if !input.is_empty()
            && input
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            Ok(input.to_owned())
        } else {
            Err(Error::InvalidRedirect)
        };
    }
    let url = url::Url::parse(input).map_err(|_| Error::InvalidRedirect)?;
    url.query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
        .filter(|c| !c.is_empty())
        .ok_or(Error::InvalidRedirect)
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(serde::Deserialize)]
struct TokenInfo {
    user_id: String,
}

/// Trades the sign-in code for the account ID. The code works only once and
/// expires within minutes.
pub fn fetch_account_id(code: &str) -> Result<AccountId> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let auth = format!(
        "Basic {}",
        BASE64.encode(format!("{CLIENT_ID}:{CLIENT_SECRET}"))
    );

    let token: TokenResponse = agent
        .post(TOKEN_URL)
        .header("Authorization", &auth)
        .send_form([
            ("grant_type", "authorization_code"),
            ("code", code),
            ("scope", SCOPE),
            ("redirect_uri", REDIRECT_URI),
        ])
        .map_err(psn_error)?
        .body_mut()
        .read_json()
        .map_err(psn_error)?;

    let info: TokenInfo = agent
        .get(&format!("{TOKEN_URL}/{}", token.access_token))
        .header("Authorization", &auth)
        .call()
        .map_err(psn_error)?
        .body_mut()
        .read_json()
        .map_err(psn_error)?;

    let user_id: u64 = info
        .user_id
        .parse()
        .map_err(|_| Error::Psn(format!("unexpected user_id {:?}", info.user_id)))?;
    Ok(AccountId::from_user_id(user_id))
}

fn psn_error(e: ureq::Error) -> Error {
    match e {
        ureq::Error::StatusCode(400 | 401) => Error::Psn(
            "PSN rejected the sign-in code; it works only once and expires quickly, so sign in again".into(),
        ),
        other => Error::Psn(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_id_from_user_id() {
        // user_id 1 → 01 00 00 00 00 00 00 00 → "AQAAAAAAAAA="
        let id = AccountId::from_user_id(1);
        assert_eq!(id.to_base64(), "AQAAAAAAAAA=");
        assert_eq!("AQAAAAAAAAA=".parse::<AccountId>().unwrap(), id);
        let big = AccountId::from_user_id(0x0123_4567_89ab_cdef);
        assert_eq!(
            big.as_bytes(),
            &[0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01]
        );
        assert_eq!(big.to_string().parse::<AccountId>().unwrap(), big);
    }

    #[test]
    fn account_id_rejects_wrong_length() {
        assert!("AQAAAA==".parse::<AccountId>().is_err());
        assert!("not base64!".parse::<AccountId>().is_err());
    }

    #[test]
    fn redirect_parsing() {
        let url = "https://remoteplay.dl.playstation.net/remoteplay/redirect?code=Ab12Cd&cid=x";
        assert_eq!(parse_redirect(url).unwrap(), "Ab12Cd");
        assert_eq!(parse_redirect("  Ab12Cd \n").unwrap(), "Ab12Cd");
        assert!(parse_redirect("https://example.com/?foo=bar").is_err());
        assert!(parse_redirect("").is_err());
    }

    #[test]
    fn login_url_has_client_and_redirect() {
        let url = url::Url::parse(&login_url()).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], CLIENT_ID);
        assert_eq!(q["redirect_uri"], REDIRECT_URI);
        assert_eq!(q["response_type"], "code");
    }
}
