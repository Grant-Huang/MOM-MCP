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
    let database = env::var("MOM_ODOO_DATABASE").unwrap_or_default();
    let company = env::var("MOM_CARBON_COMPANY").unwrap_or_default();
    let project_key = env::var("MOM_CARBON_PROJECT_KEY").unwrap_or_default();
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
    let source = HttpOrderReader::new(HttpSource {
        kind,
        base_url: endpoint,
        token: &token,
        tenant: "sample-tenant",
        site: "sample-site",
        instance: vendor,
    })
    .map_err(|_| "Invalid sample connection")?;
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
    if source.read_order(hidden).await != Err(ReadError::NotVisible) {
        return Err("Out-of-scope fixture did not return NotVisible");
    }
    println!("{vendor}: visible record projected, out-of-scope record not visible");
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
