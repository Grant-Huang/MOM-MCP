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
        calls.lock().unwrap().push(format!("{model}:{id}"));
        let (fields, rows) = match (model.as_str(), id) {
            ("mrp.production", id @ 42..=47) => (
                json!(["id", "company_id", "sale_line_id"]),
                json!([{"id": id, "company_id": [7, "Company"],
                    "sale_line_id": if id == 43 { json!(false) } else { json!([id + 100, "Line"]) }}]),
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
            "mrp.production:43",
            "mrp.production:44",
            "sale.order.line:144",
            "mrp.production:45",
            "sale.order.line:145",
            "mrp.production:46",
            "sale.order.line:146",
            "mrp.production:47",
            "sale.order.line:147",
        ]
    );
    server.abort();
}
