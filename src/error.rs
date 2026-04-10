use thiserror::Error;

#[derive(Debug, Error)]
pub enum CalDavError {
    #[error("authentication failure: {0}")]
    AuthFailure(String),

    #[error("resource not found: {0}")]
    ResourceNotFound(String),

    #[error("server overloaded (retry later)")]
    ServerOverloaded,

    #[error("XML parse error: {0}")]
    XmlParseError(String),

    #[error("ICS parse error: {0}")]
    IcsParseError(String),

    #[error("partial failure: {errors:?}")]
    PartialFailure {
        errors: Vec<String>,
    },

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("configuration error: {0}")]
    Config(String),
}

impl From<CalDavError> for rmcp::ErrorData {
    fn from(err: CalDavError) -> Self {
        let code = match &err {
            CalDavError::AuthFailure(_) => rmcp::model::ErrorCode(-32001),
            CalDavError::ResourceNotFound(_) => rmcp::model::ErrorCode(-32002),
            CalDavError::ServerOverloaded => rmcp::model::ErrorCode(-32003),
            CalDavError::XmlParseError(_) => rmcp::model::ErrorCode(-32004),
            CalDavError::IcsParseError(_) => rmcp::model::ErrorCode(-32005),
            CalDavError::PartialFailure { .. } => rmcp::model::ErrorCode(-32006),
            CalDavError::Http(_) => rmcp::model::ErrorCode(-32007),
            CalDavError::Config(_) => rmcp::model::ErrorCode(-32008),
        };
        rmcp::ErrorData::new(code, err.to_string(), None::<serde_json::Value>)
    }
}

impl rmcp::model::IntoContents for CalDavError {
    fn into_contents(self) -> Vec<rmcp::model::Content> {
        vec![rmcp::model::Content::text(self.to_string())]
    }
}

impl CalDavError {
    pub fn from_http_status(status: reqwest::StatusCode, url: &str) -> Option<Self> {
        match status.as_u16() {
            401 | 403 => Some(CalDavError::AuthFailure(format!("{status} for {url}"))),
            404 => Some(CalDavError::ResourceNotFound(url.to_string())),
            429 | 503 => Some(CalDavError::ServerOverloaded),
            _ => None,
        }
    }
}
