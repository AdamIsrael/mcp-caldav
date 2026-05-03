use serde::Deserialize;
use std::path::PathBuf;

use crate::error::CalDavError;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub name: String,
    pub url: String,
    pub auth_type: AuthType,
    pub credentials: Credentials,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum AuthType {
    Basic,
    AppPassword,
    OAuth2,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Credentials {
    pub user: Option<String>,
    pub pass: Option<String>,
    pub token: Option<String>,
    #[allow(dead_code)] // reserved for OAuth2 refresh flow
    pub refresh_token: Option<String>,
}

impl AppConfig {
    pub fn load() -> Result<Self, CalDavError> {
        let path = config_path()?;
        let contents = std::fs::read_to_string(&path).map_err(|e| {
            CalDavError::Config(format!("failed to read config at {}: {e}", path.display()))
        })?;
        let config: AppConfig = toml::from_str(&contents)
            .map_err(|e| CalDavError::Config(format!("invalid config: {e}")))?;

        for account in &config.accounts {
            account.validate()?;
        }

        Ok(config)
    }
}

fn config_path() -> Result<PathBuf, CalDavError> {
    if let Ok(path) = std::env::var("MCP_CALDAV_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var("HOME")
        .map_err(|_| CalDavError::Config("HOME not set".to_string()))?;
    Ok(PathBuf::from(home).join(".config/mcp-caldav/config.toml"))
}

impl Account {
    fn validate(&self) -> Result<(), CalDavError> {
        match self.auth_type {
            AuthType::Basic | AuthType::AppPassword => {
                if self.credentials.user.is_none() || self.credentials.pass.is_none() {
                    return Err(CalDavError::Config(format!(
                        "account '{}': Basic/AppPassword auth requires 'user' and 'pass'",
                        self.name
                    )));
                }
            }
            AuthType::OAuth2 => {
                if self.credentials.token.is_none() {
                    return Err(CalDavError::Config(format!(
                        "account '{}': OAuth2 auth requires 'token'",
                        self.name
                    )));
                }
            }
        }
        Ok(())
    }
}
