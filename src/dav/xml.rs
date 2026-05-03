use serde::Deserialize;

use crate::error::CalDavError;

// --- Response types for WebDAV multistatus ---

#[derive(Debug, Deserialize)]
#[serde(rename = "multistatus")]
pub struct Multistatus {
    #[serde(rename = "response", default)]
    pub responses: Vec<DavResponse>,
}

#[derive(Debug, Deserialize)]
pub struct DavResponse {
    pub href: String,
    #[serde(rename = "propstat", default)]
    pub propstats: Vec<Propstat>,
}

#[derive(Debug, Deserialize)]
pub struct Propstat {
    pub prop: Prop,
    pub status: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct Prop {
    pub displayname: Option<String>,
    #[serde(rename = "calendar-data")]
    pub calendar_data: Option<String>,
    #[serde(rename = "calendar-color")]
    pub calendar_color: Option<String>,
    pub resourcetype: Option<ResourceType>,
    #[allow(dead_code)] // parsed for future content-type checks
    pub getcontenttype: Option<String>,
    pub getetag: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ResourceType {
    #[allow(dead_code)] // parsed but not currently inspected; calendar field is what we use
    pub collection: Option<Empty>,
    pub calendar: Option<Empty>,
}

#[derive(Debug, Deserialize)]
pub struct Empty;

/// A successfully parsed resource from a 207 multistatus response.
#[derive(Debug, Clone)]
pub struct DavResource {
    pub href: String,
    pub displayname: Option<String>,
    pub calendar_data: Option<String>,
    pub calendar_color: Option<String>,
    pub is_calendar: bool,
    #[allow(dead_code)] // captured for future conditional-GET / If-Match support
    pub etag: Option<String>,
}

/// Parse a WebDAV 207 multistatus XML response.
/// Returns successful resources and logs partial failures.
pub fn parse_multistatus(xml: &str) -> Result<Vec<DavResource>, CalDavError> {
    let ms: Multistatus = quick_xml::de::from_str(xml)
        .map_err(|e| CalDavError::XmlParseError(format!("multistatus parse: {e}")))?;

    let mut resources = Vec::new();
    let mut errors = Vec::new();

    for response in ms.responses {
        let mut success_prop: Option<Prop> = None;
        let mut had_success = false;

        for propstat in response.propstats {
            if propstat.status.contains("200") {
                success_prop = Some(propstat.prop);
                had_success = true;
            } else if !propstat.status.contains("404") {
                // 404 on a property is normal (property not supported), skip it.
                // Other errors are real partial failures.
                errors.push(format!("{}: {}", response.href, propstat.status));
            }
        }

        if had_success {
            let prop = success_prop.unwrap_or_default();
            let is_calendar = prop
                .resourcetype
                .as_ref()
                .is_some_and(|rt| rt.calendar.is_some());

            resources.push(DavResource {
                href: response.href.clone(),
                displayname: prop.displayname,
                calendar_data: prop.calendar_data,
                calendar_color: prop.calendar_color,
                is_calendar,
                etag: prop.getetag,
            });
        }
    }

    if !errors.is_empty() {
        tracing::warn!("partial failures in multistatus: {:?}", errors);
    }

    Ok(resources)
}

// --- XML request body builders ---

pub fn propfind_calendars_body() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:a="http://apple.com/ns/ical/">
  <d:prop>
    <d:displayname/>
    <d:resourcetype/>
    <a:calendar-color/>
  </d:prop>
</d:propfind>"#
}

pub fn calendar_query_body(start: &str, end: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:time-range start="{start}" end="{end}"/>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#
    )
}

#[allow(dead_code)] // helper retained for callers that need calendar-multiget instead of inlined calendar-data
pub fn multiget_body(urls: &[&str]) -> String {
    let hrefs: String = urls
        .iter()
        .map(|u| format!("  <d:href>{u}</d:href>"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<c:calendar-multiget xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
{hrefs}
</c:calendar-multiget>"#
    )
}

pub fn uid_query_body(uid: &str) -> String {
    let uid = uid
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:prop-filter name="UID">
          <c:text-match collation="i;unicode-casemap" match-type="equals">{uid}</c:text-match>
        </c:prop-filter>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#
    )
}
