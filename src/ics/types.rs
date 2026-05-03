use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CalendarInfo {
    pub name: String,
    pub url: String,
    pub account_name: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventSummary {
    pub uid: String,
    pub summary: String,
    pub dtstart: DateTime<Utc>,
    pub dtend: Option<DateTime<Utc>>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub is_recurring: bool,
    pub url: String,
}

impl EventSummary {
    /// Case-insensitive match on summary, description, or location.
    /// An empty query matches everything.
    pub fn matches_query(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let q = query.to_lowercase();
        self.summary.to_lowercase().contains(&q)
            || self
                .description
                .as_deref()
                .is_some_and(|d| d.to_lowercase().contains(&q))
            || self
                .location
                .as_deref()
                .is_some_and(|l| l.to_lowercase().contains(&q))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EventDetail {
    pub uid: String,
    pub summary: String,
    pub description: Option<String>,
    pub dtstart: DateTime<Utc>,
    pub dtend: Option<DateTime<Utc>>,
    pub location: Option<String>,
    pub organizer: Option<String>,
    pub attendees: Vec<String>,
    pub rrule: Option<String>,
    pub is_recurring: bool,
    pub url: String,
    pub local_start: String,
    pub local_end: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventInstance {
    pub master_uid: String,
    pub summary: String,
    pub instance_start: DateTime<Utc>,
    pub instance_end: DateTime<Utc>,
    pub local_start: String,
    pub local_end: String,
    pub location: Option<String>,
    pub description: Option<String>,
}

impl std::fmt::Display for CalendarInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "- {} [{}] ({})", self.name, self.account_name, self.url)
    }
}

impl std::fmt::Display for EventSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let end = self
            .dtend
            .map(|e| format!(" -> {}", e.format("%Y-%m-%d %H:%M UTC")))
            .unwrap_or_default();
        let recurring = if self.is_recurring { " (recurring)" } else { "" };
        write!(
            f,
            "- {} | {}{}{} [{}]",
            self.summary,
            self.dtstart.format("%Y-%m-%d %H:%M UTC"),
            end,
            recurring,
            self.uid,
        )
    }
}

impl std::fmt::Display for EventInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "- {} | {} -> {} (instance of {})",
            self.summary, self.local_start, self.local_end, self.master_uid,
        )
    }
}

impl std::fmt::Display for EventDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Event: {}", self.summary)?;
        writeln!(f, "  UID: {}", self.uid)?;
        writeln!(f, "  Start: {} ({})", self.local_start, self.dtstart.format("%Y-%m-%d %H:%M UTC"))?;
        if let Some(end) = &self.local_end {
            writeln!(f, "  End: {} ({} UTC)", end, self.dtend.map(|e| e.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default())?;
        }
        if let Some(loc) = &self.location {
            writeln!(f, "  Location: {loc}")?;
        }
        if let Some(desc) = &self.description {
            writeln!(f, "  Description: {desc}")?;
        }
        if let Some(org) = &self.organizer {
            writeln!(f, "  Organizer: {org}")?;
        }
        if !self.attendees.is_empty() {
            writeln!(f, "  Attendees: {}", self.attendees.join(", "))?;
        }
        if let Some(rrule) = &self.rrule {
            writeln!(f, "  Recurrence: {rrule}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(summary: &str, description: Option<&str>, location: Option<&str>) -> EventSummary {
        EventSummary {
            uid: "u".into(),
            summary: summary.into(),
            dtstart: Utc::now(),
            dtend: None,
            location: location.map(String::from),
            description: description.map(String::from),
            is_recurring: false,
            url: String::new(),
        }
    }

    #[test]
    fn matches_summary() {
        assert!(ev("Standup with team", None, None).matches_query("standup"));
    }

    #[test]
    fn matches_description_only() {
        // Query word lives in description; not present in summary or location.
        let e = ev("Weekly sync", Some("Discuss roadmap and dragonfruit plans"), None);
        assert!(!e.summary.to_lowercase().contains("dragonfruit"));
        assert!(e.matches_query("dragonfruit"));
    }

    #[test]
    fn matches_location_only() {
        let e = ev("Lunch", None, Some("Cafe Aurelius, Berlin"));
        assert!(!e.summary.to_lowercase().contains("aurelius"));
        assert!(e.matches_query("aurelius"));
    }

    #[test]
    fn case_insensitive() {
        let e = ev("Quarterly Review", None, None);
        assert!(e.matches_query("QUARTERLY"));
        assert!(e.matches_query("quarterly"));
        assert!(e.matches_query("ReVieW"));
    }

    #[test]
    fn no_match_returns_false() {
        let e = ev("Standup", Some("notes"), Some("room 3"));
        assert!(!e.matches_query("xyzzy"));
    }

    #[test]
    fn empty_query_matches_everything() {
        let e = ev("anything", None, None);
        assert!(e.matches_query(""));
    }

    #[test]
    fn does_not_match_uid_or_other_fields() {
        // Regression: previous implementation matched against the full ICS text,
        // so a query like "MO" would hit BYDAY=MO inside an RRULE, or match
        // against a UID. The structured matcher must ignore those fields.
        let e = EventSummary {
            uid: "dragonfruit-uid-123".into(),
            summary: "Standup".into(),
            dtstart: Utc::now(),
            dtend: None,
            location: None,
            description: None,
            is_recurring: false,
            url: String::new(),
        };
        assert!(!e.matches_query("dragonfruit"));
    }
}
