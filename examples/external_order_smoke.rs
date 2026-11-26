//! Read-only smoke runner for operator-supplied synthetic sample records.

use mom_mcp::external_http::{HttpOrderReader, HttpSource, HttpSourceKind, ReadError};
use std::{env, process::ExitCode};

async fn check() -> Result<(), &'static str> {
    let args: Vec<_> = env::args().skip(1).collect();
    let [vendor, endpoint, visible, hidden, expected_status] = args.as_slice() else {
        return Err("Expected: vendor endpoint visible_id hidden_id expected_source_status");
    };
    if visible == hidden {
        return Err("Sample IDs must differ");
    }
    let token = env::var("MOM_SAMPLE_TOKEN").map_err(|_| "Missing MOM_SAMPLE_TOKEN")?;
    let witness_token = env::var("MOM_ODOO_WITNESS_TOKEN").ok();
    if let Some(witness) = &witness_token {
        if vendor != "odoo" || witness.trim().is_empty() || witness == &token {
            return Err("Odoo witness requires a distinct, nonempty read-only token");
        }
    }
    let database = env::var("MOM_ODOO_DATABASE").unwrap_or_default();
    let company = env::var("MOM_CARBON_COMPANY").unwrap_or_default();
    let project_key = env::var("MOM_CARBON_PROJECT_KEY").unwrap_or_default();
    let reader = |token| {
        let kind = match vendor.as_str() {
            "odoo" => HttpSourceKind::Odoo {
                database: &database,
            },
            "openmes" => HttpSourceKind::OpenMes,
            "carbon-rest" => HttpSourceKind::CarbonRest {
                company: &company,
                project_key: &project_key,
            },
            _ => return Err("Expected odoo, openmes or carbon-rest"),
        };
        HttpOrderReader::new(HttpSource {
            kind,
            base_url: endpoint,
            token,
            tenant: "sample-tenant",
            site: "sample-site",
            instance: vendor,
        })
        .map_err(|_| "Invalid sample connection")
    };
    let source = reader(token.as_str())?;
    let order = source
        .read_order(visible)
        .await
        .map_err(|error| match error {
            ReadError::SourceUnavailable => "Visible record: source unavailable",
            ReadError::NotVisible => "Visible record: not visible",
            _ => "Visible record: invalid response or binding",
        })?;
    if order.identity.source_id != visible
        || order.source_status.as_deref() != Some(expected_status)
        || order.normalized_status.is_some()
    {
        return Err("Visible record did not match the expected observation");
    }
    if let Ok(expected) = env::var("MOM_SAMPLE_SOURCE_UPDATED_AT") {
        let timestamp = order.source_updated_at.as_deref();
        let matches = if expected.is_empty() {
            timestamp.is_none()
        } else {
            let expected = chrono::DateTime::parse_from_rfc3339(&expected)
                .map_err(|_| "Expected source update time must be RFC3339")?;
            timestamp.is_some_and(|value| {
                value.ends_with('Z') && chrono::DateTime::parse_from_rfc3339(value) == Ok(expected)
            })
        };
        if !matches {
            return Err("Visible record did not match the expected source update time");
        }
    }
    if source.read_order(hidden).await != Err(ReadError::NotVisible) {
        return Err("Out-of-scope fixture did not return NotVisible");
    }
    if let Some(witness) = witness_token.as_deref() {
        reader(witness)?
            .read_order(hidden)
            .await
            .map_err(|_| "Odoo witness could not confirm the out-of-scope record")?;
        println!("odoo: visible record projected, scope isolation independently confirmed");
    } else {
        println!("{vendor}: visible record projected, out-of-scope record not visible");
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match check().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("{reason}");
            ExitCode::FAILURE
        }
    }
}
