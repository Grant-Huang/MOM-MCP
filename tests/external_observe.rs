use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use mom_mcp::{
    external_http::{HttpSource, HttpSourceKind},
    external_observe::{
        AuthorizedWorkOrder, ConfigurationError, ExternalObserve, ObserveError, SourceScope,
        WorkOrderSelection,
    },
};
use serde_json::json;

fn source<'a>(url: &'a str, site: &'a str, token: &'a str) -> HttpSource<'a> {
    HttpSource {
        kind: HttpSourceKind::OpenMes,
        base_url: url,
        token,
        tenant: "tenant-a",
        site,
        instance: "openmes",
    }
}

fn granted<'a>(site: &'a str, source_id: &'a str) -> AuthorizedWorkOrder<'a> {
    AuthorizedWorkOrder {
        scope: SourceScope {
            tenant: "tenant-a",
            site,
            instance: "openmes",
        },
        source_id,
    }
}

async fn run_source(status: StatusCode) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    async fn order(
        State((calls, status)): State<(Arc<AtomicUsize>, StatusCode)>,
        Path(id): Path<u64>,
    ) -> impl IntoResponse {
        calls.fetch_add(1, Ordering::Relaxed);
        assert_eq!(id, 4242);
        (
            status,
            Json(json!({"data":{"id":id,"order_no":"WO-42","status":"PENDING"}})),
        )
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/api/v1/work-orders/{id}", get(order))
        .with_state((calls.clone(), status));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (origin, calls, task)
}

#[tokio::test]
async fn resolves_each_request_before_read_and_exposes_only_observation_fields() {
    let (origin, calls, task) = run_source(StatusCode::OK).await;
    let service = ExternalObserve::new([source(&origin, "site-a", "local-fixture")]).unwrap();
    let selection = WorkOrderSelection {
        world_id: "world-a",
        object_ref: "opaque-ref",
    };

    let denied = service.observe(selection, |_| async { None }).await;
    assert_eq!(denied.unwrap_err(), ObserveError::AccessDenied);
    assert_eq!(calls.load(Ordering::Relaxed), 0);

    let observed = service
        .observe(selection, |requested| async move {
            assert_eq!(requested.world_id, "world-a");
            assert_eq!(requested.object_ref, "opaque-ref");
            Some(granted("site-a", "4242"))
        })
        .await
        .unwrap();
    assert_eq!(observed.object_ref, "opaque-ref");
    assert_eq!(observed.display_number, "WO-42");
    assert_eq!(observed.source_status.as_deref(), Some("PENDING"));
    let serialized = serde_json::to_string(&observed).unwrap();
    for private in ["4242", "tenant-a", "site-a", "local-fixture", &origin] {
        assert!(!serialized.contains(private));
    }
    let revoked = service.observe(selection, |_| async { None }).await;
    assert_eq!(revoked.unwrap_err(), ObserveError::AccessDenied);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    task.abort();
}

#[tokio::test]
async fn unavailable_or_unconfigured_binding_never_reads_another_source() {
    let (primary, primary_calls, primary_task) = run_source(StatusCode::SERVICE_UNAVAILABLE).await;
    let (alternate, alternate_calls, alternate_task) = run_source(StatusCode::OK).await;
    let service = ExternalObserve::new([
        source(&primary, "site-a", "primary"),
        source(&alternate, "site-b", "alternate"),
    ])
    .unwrap();
    let selection = WorkOrderSelection {
        world_id: "world-a",
        object_ref: "opaque-ref",
    };
    for site in ["site-a", "unconfigured"] {
        let error = service
            .observe(selection, |_| async { Some(granted(site, "4242")) })
            .await;
        assert_eq!(error.unwrap_err(), ObserveError::SourceUnavailable);
    }
    assert_eq!(primary_calls.load(Ordering::Relaxed), 1);
    assert_eq!(alternate_calls.load(Ordering::Relaxed), 0);
    primary_task.abort();
    alternate_task.abort();
}

#[test]
fn rejects_duplicate_configuration() {
    let origin = "https://example.test";
    assert!(matches!(
        ExternalObserve::new([source(origin, "site-a", "a"), source(origin, "site-a", "b")]),
        Err(ConfigurationError::DuplicateSource)
    ));
}

#[tokio::test]
async fn one_observe_entry_reads_three_authorized_worlds() {
    use axum::routing::post;

    async fn odoo(Json(request): Json<serde_json::Value>) -> Json<serde_json::Value> {
        assert_eq!(request["domain"][0][2], 4242);
        Json(json!([{"id":4242,"name":"WO-42","state":"confirmed"}]))
    }

    async fn carbon(Json(request): Json<serde_json::Value>) -> impl IntoResponse {
        let id = &request["id"];
        let result = match request["method"].as_str().unwrap() {
            "initialize" => json!({"protocolVersion":"2025-11-25"}),
            "notifications/initialized" => return StatusCode::ACCEPTED.into_response(),
            "tools/call" => {
                assert_eq!(request["params"]["arguments"]["name"], "production_getJob");
                assert_eq!(
                    request["params"]["arguments"]["arguments"]["id"],
                    "job-4242"
                );
                json!({"content":[{"type":"text","text":json!({
                    "id":"job-4242","jobId":"WO-42","status":"Ready"
                }).to_string()}]})
            }
            _ => panic!("unexpected method"),
        };
        Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
    }

    let router = Router::new()
        .route("/json/2/mrp.production/search_read", post(odoo))
        .route(
            "/api/v1/work-orders/4242",
            get(|| async {
                Json(json!({"data":{"id":4242,"order_no":"WO-42","status":"PENDING"}}))
            }),
        )
        .route("/api/mcp", post(carbon));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let connectors = [
        (
            "odoo",
            HttpSourceKind::Odoo {
                database: "fixture",
            },
        ),
        ("openmes", HttpSourceKind::OpenMes),
        ("carbon", HttpSourceKind::Carbon),
    ];
    let service = ExternalObserve::new(connectors.map(|(instance, kind)| HttpSource {
        instance,
        kind,
        ..source(&origin, "site-a", "local-fixture")
    }))
    .unwrap();

    for (world, expected_instance, expected_state) in [
        ("world-a", "odoo", "confirmed"),
        ("world-b", "openmes", "PENDING"),
        ("world-c", "carbon", "Ready"),
    ] {
        let observation = service
            .observe(
                WorkOrderSelection {
                    world_id: world,
                    object_ref: "opaque-42",
                },
                |selection| async move {
                    let (instance, source_id) = match (selection.world_id, selection.object_ref) {
                        ("world-a", "opaque-42") => ("odoo", "4242"),
                        ("world-b", "opaque-42") => ("openmes", "4242"),
                        ("world-c", "opaque-42") => ("carbon", "job-4242"),
                        _ => return None,
                    };
                    Some(AuthorizedWorkOrder {
                        scope: SourceScope {
                            tenant: "tenant-a",
                            site: "site-a",
                            instance,
                        },
                        source_id,
                    })
                },
            )
            .await
            .unwrap();
        assert_eq!(observation.object_ref, "opaque-42");
        assert_eq!(observation.display_number, "WO-42");
        assert_eq!(observation.source_instance, expected_instance);
        assert_eq!(observation.source_status.as_deref(), Some(expected_state));
        assert_eq!(observation.normalized_status, None);
    }
    task.abort();
}
