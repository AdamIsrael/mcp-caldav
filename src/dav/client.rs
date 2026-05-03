use reqwest::{Client, Method, StatusCode};

use crate::auth::apply_auth;
use crate::config::Account;
use crate::dav::xml::{self, DavResource};
use crate::error::CalDavError;
use crate::ics::types::CalendarInfo;

#[derive(Debug, Clone)]
pub struct WebDavClient {
    http: Client,
    account: Account,
}

impl WebDavClient {
    pub fn new(account: Account) -> Self {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("failed to build HTTP client");
        Self { http, account }
    }

    pub fn account_name(&self) -> &str {
        &self.account.name
    }

    pub fn base_url(&self) -> &str {
        &self.account.url
    }

    /// Send a PROPFIND request.
    pub async fn propfind(&self, url: &str, depth: u8, body: &str) -> Result<Vec<DavResource>, CalDavError> {
        let req = self
            .http
            .request(Method::from_bytes(b"PROPFIND").unwrap(), url)
            .header("Depth", depth.to_string())
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body.to_string());

        let req = apply_auth(req, &self.account);
        let resp = req.send().await?;
        self.check_status(resp.status(), url)?;

        let text = resp.text().await?;
        xml::parse_multistatus(&text)
    }

    /// Send a REPORT request (calendar-query, calendar-multiget, etc).
    pub async fn report(&self, url: &str, body: &str) -> Result<Vec<DavResource>, CalDavError> {
        let req = self
            .http
            .request(Method::from_bytes(b"REPORT").unwrap(), url)
            .header("Depth", "1")
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body.to_string());

        let req = apply_auth(req, &self.account);
        let resp = req.send().await?;
        self.check_status(resp.status(), url)?;

        let text = resp.text().await?;
        xml::parse_multistatus(&text)
    }

    /// GET a single resource (ICS file).
    pub async fn get(&self, url: &str) -> Result<String, CalDavError> {
        let req = self.http.get(url);
        let req = apply_auth(req, &self.account);
        let resp = req.send().await?;
        self.check_status(resp.status(), url)?;
        Ok(resp.text().await?)
    }

    /// Discover calendars under the account's base URL.
    pub async fn discover_calendars(&self) -> Result<Vec<CalendarInfo>, CalDavError> {
        let resources = self
            .propfind(&self.account.url, 1, xml::propfind_calendars_body())
            .await?;

        let calendars = resources
            .into_iter()
            .filter(|r| r.is_calendar)
            .map(|r| {
                let url = resolve_href(&self.account.url, &r.href);
                CalendarInfo {
                    name: r.displayname.unwrap_or_else(|| "Untitled".to_string()),
                    url,
                    account_name: self.account.name.clone(),
                    color: r.calendar_color,
                }
            })
            .collect();

        Ok(calendars)
    }

    fn check_status(&self, status: StatusCode, url: &str) -> Result<(), CalDavError> {
        if status.is_success() || status == StatusCode::MULTI_STATUS {
            return Ok(());
        }
        if let Some(err) = CalDavError::from_http_status(status, url) {
            return Err(err);
        }
        Err(CalDavError::XmlParseError(format!("unexpected HTTP {status} for {url}")))
    }
}

/// Resolve a potentially relative href against a base URL.
fn resolve_href(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    // Extract scheme + host from base
    if let Some(pos) = base.find("://")
        && let Some(slash) = base[pos + 3..].find('/')
    {
        let origin = &base[..pos + 3 + slash];
        return format!("{origin}{href}");
    }
    // Fallback: just concatenate
    format!("{base}{href}")
}
