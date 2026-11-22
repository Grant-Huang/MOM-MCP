use axum::{
    Json, Router,
    body::Body,
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use mom_mcp::external_http::{HttpOrderReader, HttpSource, HttpSourceKind, ReadError};
use serde_json::{Value, json};

async fn odoo(headers: HeaderMap, Json(request): Json<Value>) -> impl IntoResponse {
    assert_eq!(headers["authorization"], "Bearer local-fixture");
    assert_eq!(headers["x-odoo-database"], "sample-db");
    let id = request["domain"][0][2].as_i64().unwrap();
    assert_eq!(
        request,
        json!({
            "domain": [["id", "=", id]], "fields": ["id", "name", "state", "write_date"], "limit": 1
        })
    );
    Json(if id == 42 {
        json!([{"id":42,"name":"MO-42","state":"confirmed"}])
    } else {
        json!([])
    })
}

async fn openmes(headers: HeaderMap, Path(id): Path<u64>) -> impl IntoResponse {
    assert_eq!(headers["authorization"], "Bearer local-fixture");
    match id {
        42 => {
            Json(json!({"data":{"id":42,"order_no":"WO-42","status":"ACCEPTED"}})).into_response()
        }
        43 => StatusCode::FORBIDDEN.into_response(),
        44 => StatusCode::NOT_FOUND.into_response(),
        45 => StatusCode::UNAUTHORIZED.into_response(),
        46 => Redirect::temporary("/api/v1/work-orders/42").into_response(),
        47 => (StatusCode::OK, Body::from(vec![b' '; 256 * 1024 + 1])).into_response(),
        48 => Json(json!({"data":{"id":42,"order_no":"WO-42"}})).into_response(),
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

fn source<'a>(base_url: &'a str, kind: HttpSourceKind<'a>) -> HttpSource<'a> {
    HttpSource {
        kind,
        base_url,
        token: "local-fixture",
        tenant: "tenant",
        site: "site",
        instance: "instance",
    }
}

#[tokio::test]
async fn reads_and_classifies_odoo_and_openmes_http_responses() {
    let router = Router::new()
        .route("/json/2/mrp.production/search_read", post(odoo))
        .route("/api/v1/work-orders/{id}", get(openmes));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let odoo = HttpOrderReader::new(source(
        &url,
        HttpSourceKind::Odoo {
            database: "sample-db",
        },
    ))
    .unwrap();
    let openmes = HttpOrderReader::new(source(&url, HttpSourceKind::OpenMes)).unwrap();

    assert_eq!(odoo.read_order("42").await.unwrap().display_number, "MO-42");
    assert_eq!(odoo.read_order("43").await, Err(ReadError::NotVisible));
    assert_eq!(
        openmes
            .read_order("42")
            .await
            .unwrap()
            .source_status
            .as_deref(),
        Some("ACCEPTED")
    );
    for (id, expected) in [
        ("43", ReadError::NotVisible),
        ("44", ReadError::NotVisible),
        ("45", ReadError::SourceUnavailable),
        ("46", ReadError::SourceUnavailable),
        ("47", ReadError::InvalidResponse),
        ("48", ReadError::UnexpectedRecord),
        ("49", ReadError::SourceUnavailable),
        ("42/../43", ReadError::InvalidBinding),
    ] {
        assert_eq!(openmes.read_order(id).await, Err(expected), "fixture {id}");
    }
    server.abort();
}

#[test]
fn rejects_cleartext_remote_sources_and_non_origin_urls() {
    for url in [
        "http://example.test",
        "https://user:secret@example.test",
        "https://example.test/api",
    ] {
        assert!(matches!(
            HttpOrderReader::new(source(url, HttpSourceKind::OpenMes)),
            Err(ReadError::InvalidBinding)
        ));
    }
}
