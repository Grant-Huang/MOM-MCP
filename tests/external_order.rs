use mom_mcp::external_order::{BoundOrder, MappingIssue, ProjectionError, Source, project_order};
use serde_json::json;

fn bind(source: Source, instance: &str) -> BoundOrder<'_> {
    BoundOrder {
        source,
        tenant: "tenant-a",
        site: "site-a",
        instance,
        source_id: "42",
    }
}

#[test]
fn projects_three_sources_and_retains_distinct_identities() {
    let cases = [
        (
            Source::Odoo,
            "odoo",
            json!([{"id":42,"name":"WO-42","state":"confirmed","write_date":"2026-11-22 16:21:58"}]),
            "confirmed",
            None,
        ),
        (
            Source::OpenMes,
            "openmes",
            json!({"data":{"id":42,"order_no":"WO-42","status":"ACCEPTED","updated_at":"2026-11-22T17:20:00+01:00"}}),
            "ACCEPTED",
            Some("2026-11-22T16:20:00Z"),
        ),
        (
            Source::Carbon,
            "carbon",
            json!({"id":"42","jobId":"WO-42","status":"Ready","updatedAt":"2026-11-22T16:20:00Z"}),
            "Ready",
            None,
        ),
    ];
    let mut identities = Vec::new();
    for (source, instance, payload, state, timestamp) in cases {
        let row = project_order(&bind(source, instance), payload).unwrap();
        assert_eq!(row.display_number, "WO-42");
        assert_eq!(row.source_status.as_deref(), Some(state));
        assert_eq!(row.normalized_status, None);
        assert_eq!(row.mapping_issue, MappingIssue::Unmapped);
        assert_eq!(row.source_updated_at.as_deref(), timestamp);
        assert_eq!(row.identity.source_id, "42");
        identities.push(row.identity);
    }
    assert_ne!(identities[0], identities[1]);
    assert_ne!(identities[1], identities[2]);
    assert_ne!(identities[0], identities[2]);
}

#[test]
fn rejects_wrong_ids_and_distinguishes_invalid_and_invisible_responses() {
    let binding = bind(Source::Odoo, "odoo");
    let row = json!({"id":42,"name":"WO-42","state":"confirmed"});
    assert_eq!(
        project_order(&binding, json!([])),
        Err(ProjectionError::NotVisible)
    );
    assert_eq!(
        project_order(&binding, json!([row, row])),
        Err(ProjectionError::InvalidResponse)
    );
    assert_eq!(
        project_order(&binding, json!([{"id":43,"name":"WO-42"}])),
        Err(ProjectionError::UnexpectedRecord)
    );
    assert_eq!(
        project_order(&binding, json!([{"id":42.0,"name":"WO-42"}])),
        Err(ProjectionError::InvalidResponse)
    );
}

#[test]
fn distinguishes_missing_and_malformed_status_and_checks_binding() {
    let mut binding = bind(Source::Carbon, "carbon");
    assert_eq!(
        project_order(&binding, json!({"id":"42","jobId":"WO-42"}))
            .unwrap()
            .mapping_issue,
        MappingIssue::Missing
    );
    assert_eq!(
        project_order(&binding, json!({"id":"42","jobId":"WO-42","status":{}})),
        Err(ProjectionError::InvalidResponse)
    );
    binding.site = " ";
    assert_eq!(
        project_order(&binding, json!({"id":"42","jobId":"WO-42"})),
        Err(ProjectionError::InvalidBinding)
    );
}
