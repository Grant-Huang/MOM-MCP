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
    }
    assert!(matches!(
        primary
            .read_odoo_sales_relation(fixture.unlinked[0], fixture.company)
            .await,
        Ok(None)
    ));
}
