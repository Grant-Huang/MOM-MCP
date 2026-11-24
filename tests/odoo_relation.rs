use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::post,
};
use mom_mcp::external_http::{HttpOrderReader, HttpSource, HttpSourceKind, ReadError};
use serde_json::{Value, json};

#[tokio::test]
async fn reads_only_the_authorized_direct_odoo_relation() {
    type Fixture = Arc<Mutex<Vec<String>>>;

    async fn read(
        State(calls): State<Fixture>,
        Path(model): Path<String>,
        headers: HeaderMap,
        Json(query): Json<Value>,
    ) -> Json<Value> {
        assert_eq!(headers["authorization"], "Bearer local-fixture");
        assert_eq!(headers["x-odoo-database"], "sample-db");
        let id = query["domain"][0][2].as_i64().unwrap();
        let call = format!("{model}:{id}");
        let repeated = {
            let mut calls = calls.lock().unwrap();
            let repeated = calls.contains(&call);
            calls.push(call);
            repeated
        };
        let (fields, rows) = match (model.as_str(), id) {
            ("mrp.production", id @ 42..=49) => (
                json!(["id", "company_id", "sale_line_id"]),
                json!([{"id": id, "company_id": [7, "Company"],
                    "sale_line_id": if id == 43 || (id == 49 && repeated) { json!(false) } else { json!([id + 100, "Line"]) }}]),
            ),
            ("sale.order.line", 142) => (
                json!(["id", "company_id", "order_id"]),
                json!([{"id":142,"company_id":[7,"Company"],"order_id":[81,"Order"]}]),
            ),
            ("sale.order.line", 144) => (
                json!(["id", "company_id", "order_id"]),
                json!([{"id":144,"company_id":[8,"Other"],"order_id":[81,"Order"]}]),
            ),
            ("sale.order.line", 145) => (json!(["id", "company_id", "order_id"]), json!([])),
            ("sale.order.line", 146) => (
                json!(["id", "company_id", "order_id"]),
                json!([{"id":"146","company_id":[7,"Company"]}]),
            ),
            ("sale.order.line", 147) => (
                json!(["id", "company_id", "order_id"]),
                json!([{"id":148,"company_id":[7,"Company"],"order_id":[81,"Order"]}]),
            ),
            ("sale.order.line", id @ (148 | 149)) => (
                json!(["id", "company_id", "order_id"]),
                json!([{"id":id,"company_id":[7,"Company"],
                    "order_id":[if id == 148 && repeated { 82 } else { 81 },"Order"]}]),
            ),
            ("sale.order", 81) => (
                json!(["id", "company_id"]),
                json!([{"id":81,"company_id":[7,"Company"]}]),
            ),
            _ => panic!("unexpected query"),
        };
        assert_eq!(
            query,
            json!({"domain":[["id","=",id]],"fields":fields,"limit":2})
        );
        Json(rows)
    }

    let calls = Fixture::default();
    let router = Router::new()
        .route("/json/2/{model}/search_read", post(read))
        .with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let reader = HttpOrderReader::new(HttpSource {
        kind: HttpSourceKind::Odoo {
            database: "sample-db",
        },
        base_url: &origin,
        token: "local-fixture",
        tenant: "tenant",
        site: "site",
        instance: "odoo",
    })
    .unwrap();

    let relation = reader
        .read_odoo_sales_relation(42, 7)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (relation.tenant, relation.site, relation.instance),
        ("tenant", "site", "odoo")
    );
    assert_eq!(
        (
            relation.company_id,
            relation.work_order_id,
            relation.sales_line_id,
            relation.sales_order_id
        ),
        (7, 42, 142, 81)
    );
    assert_eq!(reader.read_odoo_sales_relation(43, 7).await, Ok(None));
    assert_eq!(
        reader.read_odoo_sales_relation(44, 7).await,
        Err(ReadError::NotVisible)
    );
    assert_eq!(
        reader.read_odoo_sales_relation(45, 7).await,
        Err(ReadError::NotVisible)
    );
    assert_eq!(
        reader.read_odoo_sales_relation(46, 7).await,
        Err(ReadError::InvalidResponse)
    );
    assert_eq!(
        reader.read_odoo_sales_relation(47, 7).await,
        Err(ReadError::UnexpectedRecord)
    );
    for work_order_id in [48, 49] {
        assert_eq!(
            reader.read_odoo_sales_relation(work_order_id, 7).await,
            Err(ReadError::UnexpectedRecord)
        );
    }
    assert_eq!(
        reader.read_odoo_sales_relation(0, 7).await,
        Err(ReadError::InvalidBinding)
    );
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "mrp.production:42",
            "sale.order.line:142",
            "sale.order:81",
            "sale.order.line:142",
            "mrp.production:42",
            "mrp.production:43",
            "mrp.production:44",
            "sale.order.line:144",
            "mrp.production:45",
            "sale.order.line:145",
            "mrp.production:46",
            "sale.order.line:146",
            "mrp.production:47",
            "sale.order.line:147",
            "mrp.production:48",
            "sale.order.line:148",
            "sale.order:81",
            "sale.order.line:148",
            "mrp.production:49",
            "sale.order.line:149",
            "sale.order:81",
            "sale.order.line:149",
            "mrp.production:49",
        ]
    );
    server.abort();
}

#[tokio::test]
#[ignore = "requires a running Odoo Community test instance and two authorized company identities"]
async fn reads_live_odoo_relations_with_two_company_identities() {
    use mom_mcp::external_observe::{
        AuthorizedOdooRelation, ExternalObserve, ObserveError, SourceScope, WorkOrderSelection,
    };
    #[derive(serde::Deserialize)]
    struct Fixture {
        database: String,
        token: String,
        witness_token: String,
        company: i64,
        other_company: i64,
        // Each tuple is manufacturing order, sales order line, sales order.
        visible: [i64; 3],
        hidden: [i64; 3],
        unlinked: [i64; 3],
        nested_denied: Option<[i64; 3]>,
    }

    let origin = std::env::var("MOM_ODOO_RELATION_ORIGIN").expect("missing Odoo fixture origin");
    let file = std::env::var_os("MOM_ODOO_RELATION_FIXTURE").expect("missing Odoo fixture path");
    let data = std::fs::read(file).expect("could not read Odoo fixture");
    let fixture: Fixture =
        serde_json::from_slice(&data).unwrap_or_else(|_| panic!("invalid Odoo fixture"));
    assert!(
        fixture.token != fixture.witness_token,
        "expected distinct Odoo readers"
    );
    assert_ne!(fixture.company, fixture.other_company);
    assert_ne!(fixture.visible[0], fixture.hidden[0]);
    assert_ne!(fixture.visible[0], fixture.unlinked[0]);
    assert_ne!(fixture.hidden[0], fixture.unlinked[0]);
    let reader = |token| {
        HttpOrderReader::new(HttpSource {
            kind: HttpSourceKind::Odoo {
                database: &fixture.database,
            },
            base_url: &origin,
            token,
            tenant: "test",
            site: "test",
            instance: "odoo",
        })
        .expect("invalid Odoo fixture source")
    };
    let primary = reader(fixture.token.as_str());
    let witness = reader(fixture.witness_token.as_str());
    for (reader, company, own, other) in [
        (&primary, fixture.company, fixture.visible, fixture.hidden),
        (
            &witness,
            fixture.other_company,
            fixture.hidden,
            fixture.visible,
        ),
    ] {
        let relation = reader
            .read_odoo_sales_relation(own[0], company)
            .await
            .expect("own Odoo relation could not be read")
            .expect("own Odoo relation was missing");
        assert_eq!(relation.company_id, company);
        assert!(
            [
                relation.work_order_id,
                relation.sales_line_id,
                relation.sales_order_id
            ] == own,
            "Odoo relation did not match the fixture"
        );
        assert!(matches!(
            reader.read_odoo_sales_relation(other[0], company).await,
            Err(ReadError::NotVisible)
        ));
        assert_eq!(
            reader
                .read_odoo_sales_relation_atomic(own[0], company)
                .await,
            Ok(Some(relation))
        );
        assert_eq!(
            reader
                .read_odoo_sales_relation_atomic(other[0], company)
                .await,
            Err(ReadError::NotVisible)
        );
    }
    assert!(matches!(
        primary
            .read_odoo_sales_relation(fixture.unlinked[0], fixture.company)
            .await,
        Ok(None)
    ));
    assert_eq!(
        primary
            .read_odoo_sales_relation_atomic(fixture.unlinked[0], fixture.company)
            .await,
        Ok(None)
    );

    let service = ExternalObserve::new([HttpSource {
        kind: HttpSourceKind::Odoo {
            database: &fixture.database,
        },
        base_url: &origin,
        token: &fixture.token,
        tenant: "test",
        site: "test",
        instance: "odoo",
    }])
    .unwrap();
    let selection = WorkOrderSelection {
        world_id: "test-world",
        object_ref: "opaque-work-order",
    };
    let scope = SourceScope {
        tenant: "test",
        site: "test",
        instance: "odoo",
    };
    let grant = |work_order_id| async move {
        Some(AuthorizedOdooRelation {
            scope,
            work_order_id,
            company_id: fixture.company,
        })
    };

    let published = service
        .observe_odoo_sales_relation(
            selection,
            |_| grant(fixture.visible[0]),
            |requested, target| async move {
                assert_eq!(requested.world_id, selection.world_id);
                assert_eq!(target.scope, scope);
                assert_eq!(target.company_id, fixture.company);
                assert_eq!(target.sales_order_id, fixture.visible[2]);
                Some("opaque-sales-order")
            },
        )
        .await
        .expect("could not observe Odoo relation")
        .expect("expected the fixture relation");
    assert_eq!(
        serde_json::to_value(published).unwrap(),
        json!({
            "kind": "fulfills_sales_order",
            "work_order_ref": "opaque-work-order",
            "sales_order_ref": "opaque-sales-order",
        })
    );

    assert_eq!(
        service
            .observe_odoo_sales_relation(
                selection,
                |_| grant(fixture.visible[0]),
                |_, _| async { None }
            )
            .await,
        Err(ObserveError::NotVisible)
    );

    if let Some(nested_denied) = fixture.nested_denied {
        assert_ne!(nested_denied[0], fixture.visible[0]);
        assert_ne!(nested_denied[0], fixture.hidden[0]);
        assert_ne!(nested_denied[0], fixture.unlinked[0]);
        primary
            .read_order(&nested_denied[0].to_string())
            .await
            .expect("fixture manufacturing order must be visible before expanding the relation");
        assert_eq!(
            service
                .observe_odoo_sales_relation(
                    selection,
                    |_| grant(nested_denied[0]),
                    |_, _| async {
                        panic!("source-denied sales order reached the host authorizer")
                    }
                )
                .await,
            Err(ObserveError::NotVisible)
        );
    }
}

#[tokio::test]
async fn reads_atomic_odoo_relation_with_fixed_fields_and_scoped_errors() {
    use axum::http::StatusCode;

    async fn read(headers: HeaderMap, Json(query): Json<Value>) -> (StatusCode, Json<Value>) {
        assert_eq!(headers["authorization"], "Bearer local-fixture");
        assert_eq!(headers["x-odoo-database"], "sample-db");
        let id = query["ids"][0].as_i64().unwrap();
        assert_eq!(
            query,
            json!({
                "ids": [id],
                "specification": {
                    "id": {}, "company_id": {},
                    "sale_line_id": {"fields": {
                        "id": {}, "company_id": {},
                        "order_id": {"fields": {"id": {}, "company_id": {}}}
                    }}
                }
            })
        );
        let row = |company| {
            json!({"id":id,"company_id":7,"sale_line_id":{
                "id":142,"company_id":7,"order_id":{"id":81,"company_id":company}
            }})
        };
        let (status, body) = match id {
            42 => (StatusCode::OK, json!([row(json!(7))])),
            43 => (
                StatusCode::OK,
                json!([{"id":43,"company_id":7,"sale_line_id":false}]),
            ),
            44 => (StatusCode::OK, json!([row(json!(8))])),
            45 => (StatusCode::OK, json!([row(json!("7"))])),
            46 => (StatusCode::OK, json!([{"id":42,"company_id":7}])),
            47 => (
                StatusCode::FORBIDDEN,
                json!({"name":"odoo.exceptions.AccessError","message":"private"}),
            ),
            48 => (
                StatusCode::FORBIDDEN,
                json!({"name":"other","message":"private"}),
            ),
            49 => (StatusCode::NOT_FOUND, json!({})),
            _ => panic!("unexpected query"),
        };
        (status, Json(body))
    }
    let router = Router::new().route("/json/2/mrp.production/web_read", post(read));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let reader = HttpOrderReader::new(HttpSource {
        kind: HttpSourceKind::Odoo {
            database: "sample-db",
        },
        base_url: &origin,
        token: "local-fixture",
        tenant: "tenant",
        site: "site",
        instance: "odoo",
    })
    .unwrap();

    let relation = reader
        .read_odoo_sales_relation_atomic(42, 7)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            relation.work_order_id,
            relation.sales_line_id,
            relation.sales_order_id
        ),
        (42, 142, 81)
    );
    assert_eq!(
        reader.read_odoo_sales_relation_atomic(43, 7).await,
        Ok(None)
    );
    for (id, expected) in [
        (44, ReadError::NotVisible),
        (45, ReadError::InvalidResponse),
        (46, ReadError::UnexpectedRecord),
        (47, ReadError::NotVisible),
        (48, ReadError::SourceUnavailable),
        (49, ReadError::SourceUnavailable),
        (0, ReadError::InvalidBinding),
    ] {
        assert_eq!(
            reader.read_odoo_sales_relation_atomic(id, 7).await,
            Err(expected)
        );
    }
    server.abort();
}
