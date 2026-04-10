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
    pub is_recurring: bool,
    pub url: String,
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
