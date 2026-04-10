use std::time::Duration;

use moka::future::Cache;

use crate::dav::client::WebDavClient;
use crate::error::CalDavError;
use crate::ics::types::CalendarInfo;

#[derive(Clone)]
pub struct CalendarCache {
    inner: Cache<String, Vec<CalendarInfo>>,
}

impl CalendarCache {
    pub fn new() -> Self {
        let inner = Cache::builder()
            .max_capacity(100)
            .time_to_live(Duration::from_secs(300)) // 5 minutes
            .build();
        Self { inner }
    }

    /// Get calendars for an account, discovering them if not cached.
    pub async fn get_or_discover(
        &self,
        client: &WebDavClient,
    ) -> Result<Vec<CalendarInfo>, CalDavError> {
        let key = client.account_name().to_string();

        if let Some(cached) = self.inner.get(&key).await {
            return Ok(cached);
        }

        let calendars = client.discover_calendars().await?;
        self.inner.insert(key, calendars.clone()).await;
        Ok(calendars)
    }
}
