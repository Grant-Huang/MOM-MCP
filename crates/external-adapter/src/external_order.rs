//! Read-only domain projection after transport and authorization have succeeded.

use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    Odoo,
    OpenMes,
    Carbon,
}

#[derive(Debug)]
pub struct BoundOrder<'a> {
    pub source: Source,
    pub tenant: &'a str,
    pub site: &'a str,
    pub instance: &'a str,
    pub source_id: &'a str,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct OrderIdentity<'a> {
    pub tenant: &'a str,
    pub site: &'a str,
    pub instance: &'a str,
    pub object_type: &'static str,
    pub source_id: &'a str,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingIssue {
    Missing,
    Unmapped,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct OrderObservation<'a> {
    pub identity: OrderIdentity<'a>,
    pub display_number: String,
    pub source_status: Option<String>,
    pub normalized_status: Option<String>,
    pub mapping_issue: MappingIssue,
    pub mapping_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_updated_at: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ProjectionError {
    InvalidBinding,
    InvalidResponse,
    UnexpectedRecord,
    NotVisible,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RecordId {
    Numeric(u64),
    Text(String),
}

impl RecordId {
    fn matches(&self, expected: &str) -> bool {
        match self {
            Self::Numeric(value) => *value > 0 && value.to_string() == expected,
            Self::Text(value) => value == expected,
        }
    }
}

// Carbon receives the already decoded single-record MCP text payload.
// Source authentication, HTTP errors and tool errors are classified by the caller.
pub fn project_order<'a>(
    binding: &BoundOrder<'a>,
    payload: Value,
) -> Result<OrderObservation<'a>, ProjectionError> {
    use ProjectionError::*;
    if [
        binding.tenant,
        binding.site,
        binding.instance,
        binding.source_id,
    ]
    .iter()
    .any(|value| value.trim().is_empty())
    {
        return Err(InvalidBinding);
    }

    let mut record = match (binding.source, payload) {
        (Source::Odoo, Value::Array(rows)) => {
            let mut rows = rows.into_iter();
            let first = rows.next().ok_or(NotVisible)?;
            if rows.next().is_some() {
                return Err(InvalidResponse);
            }
            first
        }
        (Source::OpenMes, Value::Object(mut envelope)) => {
            envelope.remove("data").ok_or(InvalidResponse)?
        }
        (Source::Carbon, record @ Value::Object(_)) => record,
        _ => return Err(InvalidResponse),
    };
    let fields = record.as_object_mut().ok_or(InvalidResponse)?;
    let (number_key, status_key, mapping_version, source_updated_at) = match binding.source {
        Source::Odoo => {
            let timestamp = fields
                .get("write_date")
                .and_then(Value::as_str)
                .and_then(|value| {
                    let format = "%Y-%m-%d %H:%M:%S";
                    let parsed = NaiveDateTime::parse_from_str(value, format).ok()?;
                    (value.len() == 19 && parsed.format(format).to_string() == value)
                        .then(|| parsed.and_utc().to_rfc3339_opts(SecondsFormat::Secs, true))
                });
            ("name", "state", "0.2", timestamp)
        }
        Source::OpenMes => {
            let timestamp = fields
                .get("updated_at")
                .and_then(Value::as_str)
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| {
                    value
                        .with_timezone(&Utc)
                        .to_rfc3339_opts(SecondsFormat::AutoSi, true)
                });
            ("order_no", "status", "0.1", timestamp)
        }
        Source::Carbon => ("jobId", "status", "0.1", None),
    };
    // Select only the documented fields. Unknown upstream data stays outside the projection.
    let id = fields.remove("id").ok_or(InvalidResponse)?;
    let number = fields.remove(number_key).ok_or(InvalidResponse)?;
    let status = fields.remove(status_key);
    let id: RecordId = serde_json::from_value(id).map_err(|_| InvalidResponse)?;
    let display_number: String = serde_json::from_value(number).map_err(|_| InvalidResponse)?;
    let source_status: Option<String> = status
        .map(|value| serde_json::from_value(value).map_err(|_| InvalidResponse))
        .transpose()?
        .flatten();
    if !id.matches(binding.source_id) {
        return Err(UnexpectedRecord);
    }
    if display_number.trim().is_empty() {
        return Err(InvalidResponse);
    }
    let source_status = source_status.filter(|status| !status.trim().is_empty());
    let mapping_issue = if source_status.is_some() {
        MappingIssue::Unmapped
    } else {
        MappingIssue::Missing
    };
    Ok(OrderObservation {
        identity: OrderIdentity {
            tenant: binding.tenant,
            site: binding.site,
            instance: binding.instance,
            object_type: "work_order",
            source_id: binding.source_id,
        },
        display_number,
        source_status,
        normalized_status: None,
        mapping_issue,
        mapping_version,
        source_updated_at,
    })
}
