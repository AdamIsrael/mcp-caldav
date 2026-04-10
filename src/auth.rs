use reqwest::RequestBuilder;

use crate::config::{Account, AuthType};

pub fn apply_auth(builder: RequestBuilder, account: &Account) -> RequestBuilder {
    match account.auth_type {
        AuthType::Basic | AuthType::AppPassword => {
            let user = account.credentials.user.as_deref().unwrap_or("");
            let pass = account.credentials.pass.as_deref();
            builder.basic_auth(user, pass)
        }
        AuthType::OAuth2 => {
            let token = account.credentials.token.as_deref().unwrap_or("");
            builder.bearer_auth(token)
        }
    }
}
