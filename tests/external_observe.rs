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
    assert!(
        serde_json::to_value(&observed)
            .unwrap()
            .get("source_updated_at")
            .is_none()
    );
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

#[tokio::test]
async fn source_failures_keep_their_classification_without_fallback() {
    use axum::routing::post;

    async fn openmes(
        State(calls): State<Arc<AtomicUsize>>,
        Path(id): Path<u64>,
    ) -> impl IntoResponse {
        calls.fetch_add(1, Ordering::Relaxed);
        match id {
            4242 => (StatusCode::OK, "invalid JSON with upstream-private-detail"),
            4243 => (
                StatusCode::OK,
                r#"{"data":{"id":4244,"order_no":"OTHER-WO","status":"PENDING"}}"#,
            ),
            4244 => (StatusCode::FORBIDDEN, "upstream-private-detail"),
            _ => panic!("unexpected OpenMES fixture request"),
        }
    }

    async fn odoo(
        State(calls): State<Arc<AtomicUsize>>,
        Json(query): Json<serde_json::Value>,
    ) -> impl IntoResponse {
        calls.fetch_add(1, Ordering::Relaxed);
        match query["domain"][0][2].as_u64().unwrap() {
            4242 => Json(json!([])).into_response(),
            4243 => (StatusCode::UNAUTHORIZED, "upstream-private-detail").into_response(),
            _ => panic!("unexpected Odoo fixture request"),
        }
    }

    async fn carbon(
        State(calls): State<Arc<AtomicUsize>>,
        axum::extract::Query(query): axum::extract::Query<
            std::collections::HashMap<String, String>,
        >,
    ) -> impl IntoResponse {
        calls.fetch_add(1, Ordering::Relaxed);
        assert_eq!(query["companyId"], "eq.company-a");
        match query["id"].as_str() {
            "eq.job-hidden" => Json(json!([])).into_response(),
            "eq.job-mismatch" => Json(json!([{
                "id":"job-mismatch","companyId":"company-b","jobId":"OTHER-WO","status":"Ready"
            }]))
            .into_response(),
            "eq.job-unavailable" => {
                (StatusCode::SERVICE_UNAVAILABLE, "upstream-private-detail").into_response()
            }
            _ => panic!("unexpected Carbon fixture request"),
        }
    }

    let primary_calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/api/v1/work-orders/{id}", get(openmes))
        .route("/json/2/mrp.production/search_read", post(odoo))
        .route("/rest/v1/jobs", get(carbon))
        .with_state(primary_calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let primary = format!("http://{}", listener.local_addr().unwrap());
    let primary_task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (alternate, alternate_calls, alternate_task) = run_source(StatusCode::OK).await;
    let service = ExternalObserve::new([
        source(&primary, "site-a", "primary"),
        HttpSource {
            kind: HttpSourceKind::Odoo {
                database: "fixture",
            },
            instance: "odoo",
            ..source(&primary, "site-a", "primary")
        },
        HttpSource {
            kind: HttpSourceKind::CarbonRest {
                company: "company-a",
                project_key: "fixture",
            },
            instance: "carbon-rest",
            ..source(&primary, "site-a", "primary")
        },
        source(&alternate, "site-b", "alternate"),
    ])
    .unwrap();

    use ObserveError::{InvalidSourceResponse, NotVisible, SourceUnavailable};
    for (index, (instance, source_id, expected)) in [
        ("openmes", "4242", InvalidSourceResponse),
        ("openmes", "4243", InvalidSourceResponse),
        ("openmes", "4244", NotVisible),
        ("odoo", "4242", NotVisible),
        ("odoo", "4243", SourceUnavailable),
        ("carbon-rest", "job-hidden", NotVisible),
        ("carbon-rest", "job-mismatch", InvalidSourceResponse),
        ("carbon-rest", "job-unavailable", SourceUnavailable),
    ]
    .into_iter()
    .enumerate()
    {
        let result = service
            .observe(
                WorkOrderSelection {
                    world_id: instance,
                    object_ref: "opaque-ref",
                },
                |_| async {
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
            .await;
        assert_eq!(result.unwrap_err(), expected, "{instance} {source_id}");
        assert_eq!(primary_calls.load(Ordering::Relaxed), index + 1);
        assert_eq!(alternate_calls.load(Ordering::Relaxed), 0);
    }
    primary_task.abort();
    alternate_task.abort();
}

#[tokio::test]
async fn routes_by_tenant_site_and_instance_when_work_order_ids_are_equal() {
    let (primary, primary_calls, primary_task) = run_source(StatusCode::OK).await;
    let (other_tenant, tenant_calls, tenant_task) = run_source(StatusCode::OK).await;
    let (other_instance, instance_calls, instance_task) = run_source(StatusCode::OK).await;
    let service = ExternalObserve::new([
        source(&primary, "site-a", "primary"),
        HttpSource {
            tenant: "tenant-b",
            ..source(&other_tenant, "site-a", "other-tenant")
        },
        HttpSource {
            instance: "openmes-secondary",
            ..source(&other_instance, "site-a", "other-instance")
        },
    ])
    .unwrap();
    let selection = WorkOrderSelection {
        world_id: "world-a",
        object_ref: "opaque-ref",
    };

    for (tenant, instance, expected_calls) in [
        ("tenant-a", "openmes", [1, 0, 0]),
        ("tenant-b", "openmes", [1, 1, 0]),
        ("tenant-a", "openmes-secondary", [1, 1, 1]),
        ("tenant-b", "openmes-secondary", [1, 1, 1]),
    ] {
        let result = service
            .observe(selection, |_| async {
                Some(AuthorizedWorkOrder {
                    scope: SourceScope {
                        tenant,
                        site: "site-a",
                        instance,
                    },
                    source_id: "4242",
                })
            })
            .await;
        if (tenant, instance) == ("tenant-b", "openmes-secondary") {
            assert_eq!(result.unwrap_err(), ObserveError::SourceUnavailable);
        } else {
            assert_eq!(result.unwrap().object_ref, "opaque-ref");
        }
        assert_eq!(
            [&primary_calls, &tenant_calls, &instance_calls]
                .map(|calls| calls.load(Ordering::Relaxed)),
            expected_calls
        );
    }
    for task in [primary_task, tenant_task, instance_task] {
        task.abort();
    }
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
async fn one_observe_entry_preserves_source_time_across_authorized_worlds() {
    use axum::routing::post;

    async fn odoo(Json(request): Json<serde_json::Value>) -> Json<serde_json::Value> {
        assert_eq!(request["domain"][0][2], 4242);
        Json(
            json!([{"id":4242,"name":"WO-42","state":"confirmed","write_date":"2026-11-01 08:30:00"}]),
        )
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
                    "id":"job-4242","jobId":"WO-42","status":"Ready","updatedAt":"2026-11-01T10:30:00+02:00"
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
                Json(json!({"data":{"id":4242,"order_no":"WO-42","status":"PENDING","updated_at":"2026-11-01T03:30:00-05:00"}}))
            }),
        )
        .route(
            "/rest/v1/jobs",
            get(|| async {
                Json(json!([{
                    "id":"job-4242","jobId":"WO-42","status":"Ready",
                    "companyId":"company-a","updatedAt":"2026-11-01T10:30:00+02:00"
                }]))
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
        (
            "carbon-rest",
            HttpSourceKind::CarbonRest {
                company: "company-a",
                project_key: "public-fixture",
            },
        ),
        ("carbon-mcp", HttpSourceKind::Carbon),
    ];
    let service = ExternalObserve::new(connectors.map(|(instance, kind)| HttpSource {
        instance,
        kind,
        ..source(&origin, "site-a", "local-fixture")
    }))
    .unwrap();

    let utc = Some("2026-11-01T08:30:00Z");
    for (world, expected_instance, expected_state, mapping, source_time) in [
        ("world-a", "odoo", "confirmed", "0.2", utc),
        ("world-b", "openmes", "PENDING", "0.1", utc),
        ("world-c", "carbon-rest", "Ready", "0.2", utc),
        ("world-d", "carbon-mcp", "Ready", "0.1", None),
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
                        ("world-c", "opaque-42") => ("carbon-rest", "job-4242"),
                        ("world-d", "opaque-42") => ("carbon-mcp", "job-4242"),
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
        let payload = serde_json::to_value(&observation).unwrap();
        assert_eq!(payload["mapping_version"], mapping);
        assert!(observation.observed_at.ends_with('Z'));
        if let Some(expected) = source_time {
            assert_eq!(payload["source_updated_at"], expected);
            assert_ne!(payload["source_updated_at"], payload["observed_at"]);
        } else {
            assert!(payload.get("source_updated_at").is_none());
        }
    }
    task.abort();
}

#[tokio::test]
async fn publishes_only_host_authorized_odoo_relation_references() {
    use axum::routing::post;
    use mom_mcp::external_observe::AuthorizedOdooRelation;
    use serde_json::Value;

    async fn relation(
        State(calls): State<Arc<AtomicUsize>>,
        Json(query): Json<Value>,
    ) -> impl IntoResponse {
        calls.fetch_add(1, Ordering::Relaxed);
        let id = query["ids"][0].as_i64().unwrap();
        let link = match id {
            4242 => json!({"id":6363,"company_id":7474,"order_id":{"id":8585,"company_id":7474}}),
            4243 => json!(false),
            4244 => json!({"id":6363,"company_id":7474,"order_id":{"id":8585,"company_id":9696}}),
            4245 => {
                return (StatusCode::SERVICE_UNAVAILABLE, "upstream-private-detail")
                    .into_response();
            }
            4246 => {
                return Json(json!([{"id":4242,"company_id":7474,"sale_line_id":false}]))
                    .into_response();
            }
            _ => panic!("unexpected fixture request"),
        };
        Json(json!([{"id":id,"company_id":7474,"sale_line_id":link}])).into_response()
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/json/2/mrp.production/web_read", post(relation))
        .with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let alternate_calls = Arc::new(AtomicUsize::new(0));
    let alternate_router = Router::new()
        .route("/json/2/mrp.production/web_read", post(relation))
        .with_state(alternate_calls.clone());
    let alternate_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let alternate = format!("http://{}", alternate_listener.local_addr().unwrap());
    let alternate_task = tokio::spawn(async move {
        axum::serve(alternate_listener, alternate_router)
            .await
            .unwrap()
    });
    let service = ExternalObserve::new([
        HttpSource {
            kind: HttpSourceKind::Odoo {
                database: "private-db",
            },
            instance: "instance-a",
            ..source(&origin, "site-a", "local-fixture")
        },
        HttpSource {
            kind: HttpSourceKind::Odoo {
                database: "alternate-db",
            },
            instance: "instance-a",
            ..source(&alternate, "site-b", "alternate-fixture")
        },
    ])
    .unwrap();
    let selection = WorkOrderSelection {
        world_id: "world-a",
        object_ref: "opaque-work",
    };
    let scope = SourceScope {
        tenant: "tenant-a",
        site: "site-a",
        instance: "instance-a",
    };
    let grant = |work_order_id| async move {
        Some(AuthorizedOdooRelation {
            scope,
            work_order_id,
            company_id: 7474,
        })
    };

    let denied = service
        .observe_odoo_sales_relation(
            selection,
            |_| async { None },
            |_, _| async { panic!("target resolver called without a work-order authorization") },
        )
        .await;
    assert_eq!(denied, Err(ObserveError::AccessDenied));
    assert_eq!(calls.load(Ordering::Relaxed), 0);

    let observed = service
        .observe_odoo_sales_relation(
            selection,
            |_| grant(4242),
            |requested, target| async move {
                assert_eq!(requested.world_id, "world-a");
                assert_eq!(requested.object_ref, "opaque-work");
                assert_eq!(target.scope, scope);
                assert_eq!((target.company_id, target.sales_order_id), (7474, 8585));
                Some("opaque-sales")
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(observed).unwrap(),
        json!({
            "kind":"fulfills_sales_order", "work_order_ref":"opaque-work", "sales_order_ref":"opaque-sales"
        })
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);

    for target_ref in [None, Some(" ")] {
        let result = service
            .observe_odoo_sales_relation(selection, |_| grant(4242), |_, _| async { target_ref })
            .await;
        assert_eq!(result, Err(ObserveError::NotVisible));
    }
    let absent = service
        .observe_odoo_sales_relation(
            selection,
            |_| grant(4243),
            |_, _| async { panic!("target resolver called without a direct relation") },
        )
        .await;
    assert_eq!(absent, Ok(None));
    let wrong_company = service
        .observe_odoo_sales_relation(
            selection,
            |_| grant(4244),
            |_, _| async { panic!("target resolver called for another company") },
        )
        .await;
    assert_eq!(wrong_company, Err(ObserveError::NotVisible));
    assert_eq!(calls.load(Ordering::Relaxed), 5);

    for (index, (id, expected)) in [
        (4245, ObserveError::SourceUnavailable),
        (4246, ObserveError::InvalidSourceResponse),
    ]
    .into_iter()
    .enumerate()
    {
        let result = service
            .observe_odoo_sales_relation(
                selection,
                |_| grant(id),
                |_, _| async { panic!("target resolver called after a failed source read") },
            )
            .await;
        assert_eq!(result, Err(expected));
        assert_eq!(calls.load(Ordering::Relaxed), 6 + index);
        assert_eq!(alternate_calls.load(Ordering::Relaxed), 0);
    }
    server.abort();
    alternate_task.abort();
}
