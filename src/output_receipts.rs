use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Map, Value, json};

use crate::{
    catalog::{Action, Spec, object, text},
    inventory::{LOCATIONS, QUANTITY_SCALE, id_schema},
    scenario::identifier,
};

pub(crate) fn row_schema() -> Value {
    object(
        &[
            "receipt_id",
            "lot_id",
            "item_id",
            "revision",
            "location",
            "quantity",
            "unit",
            "serial_numbers",
            "posted_cursor",
        ],
        json!({"receipt_id":id_schema(),"lot_id":id_schema(),"item_id":id_schema(),
            "revision":{"type":"integer","minimum":1},"location":id_schema(),
            "quantity":{"type":"integer","minimum":1},"unit":{"const":"each"},
            "serial_numbers":{"type":"array","items":id_schema()},"posted_cursor":{"type":"integer","minimum":1}}),
    )
}

pub(crate) fn spec() -> Spec {
    Spec {
        name: Action::ReceiveWorkOrderOutput.name(),
        description: "将已确认且检验合格的工单产出入库",
        domain: "inventory",
        effect: "write",
        input: object(
            &[
                "work_order",
                "receipt_id",
                "lot_id",
                "location",
                "expected_cursor",
                "idempotency_key",
            ],
            json!({"work_order":id_schema(),"receipt_id":id_schema(),"lot_id":id_schema(),"location":id_schema(),
                "expected_cursor":{"type":"integer","minimum":0,"maximum":i64::MAX},"idempotency_key":text()}),
        ),
        output: json!({"oneOf":[object(&["ok","work_order","output_receipt","cursor"],
            json!({"ok":{"const":true},"work_order":id_schema(),"output_receipt":row_schema(),"cursor":{"type":"integer","minimum":1}})),
            object(&["ok","code"],json!({"ok":{"const":false},"code":text()}))]}),
        preconditions: &[
            "run_open",
            "tool_enabled_in_scenario",
            "work_order_completion_confirmed",
            "quality_released",
        ],
        side_effect: "receive_work_order_output",
        errors: &[
            "INVALID_ARGUMENT",
            "WORK_ORDER_NOT_FOUND",
            "OUTPUT_NOT_RECEIVABLE",
            "OUTPUT_ALREADY_RECEIVED",
            "OUTPUT_NOT_RECONCILED",
            "LOCATION_NOT_FOUND",
            "LOT_ALREADY_ASSIGNED",
            "RECEIPT_ALREADY_EXISTS",
            "ORDER_SNAPSHOT_NOT_FOUND",
            "OUTPUT_UNIT_UNSUPPORTED",
            "WORK_ORDER_HELD",
            "QUALITY_HOLD",
            "QUALITY_SCRAPPED",
            "MATERIAL_HELD",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

pub(crate) fn read(
    tx: &Transaction<'_>,
    run: &str,
    order: &str,
) -> rusqlite::Result<Option<Value>> {
    tx.query_row(
        "SELECT s.receipt,s.lot,s.item,s.revision,s.location,s.quantity,s.unit,o.serials,s.posted_cursor
         FROM work_order_output_receipts o JOIN stock_receipts s ON s.run=o.run AND s.receipt=o.receipt
         WHERE o.run=?1 AND o.work_order=?2",
        params![run,order],
        |r| {
            let serials: String = r.get(7)?;
            let serials: Vec<String> = serde_json::from_str(&serials)
                .map_err(|error| rusqlite::Error::FromSqlConversionFailure(7,rusqlite::types::Type::Text,Box::new(error)))?;
            Ok(json!({"receipt_id":r.get::<_,String>(0)?,"lot_id":r.get::<_,String>(1)?,
                "item_id":r.get::<_,String>(2)?,"revision":r.get::<_,i64>(3)?,"location":r.get::<_,String>(4)?,
                "quantity":r.get::<_,i64>(5)? / QUANTITY_SCALE,"unit":r.get::<_,String>(6)?,
                "serial_numbers":serials,"posted_cursor":r.get::<_,i64>(8)?}))
        },
    ).optional()
}

pub(crate) fn post(
    tx: &Transaction<'_>,
    run: &str,
    order: &str,
    args: &Map<String, Value>,
    cursor: i64,
    account: &Value,
    serials: &[String],
) -> anyhow::Result<Value> {
    let receipt = args["receipt_id"].as_str().unwrap();
    let lot = args["lot_id"].as_str().unwrap();
    let location = args["location"].as_str().unwrap();
    if ![order, receipt, lot, location].into_iter().all(identifier) {
        return Ok(failure("INVALID_ARGUMENT"));
    }
    if !LOCATIONS.contains(&location) {
        return Ok(failure("LOCATION_NOT_FOUND"));
    }
    if account.get("output_receipt").is_some() {
        return Ok(failure("OUTPUT_ALREADY_RECEIVED"));
    }
    if let Some(blocking) = account["blocking"].as_str() {
        return Ok(failure(blocking));
    }
    let count = serials.len();
    if account["balanced"] != true
        || count != crate::mes::QUANTITY
        || account["wip"]["assembly_reported"].as_u64() != Some(count as u64)
    {
        return Ok(failure("OUTPUT_NOT_RECONCILED"));
    }
    let Some(snapshot) = crate::snapshots::load(tx, run, order)? else {
        return Ok(failure("ORDER_SNAPSHOT_NOT_FOUND"));
    };
    let item = &snapshot["item"];
    if item["base_unit"] != "each" {
        return Ok(failure("OUTPUT_UNIT_UNSUPPORTED"));
    }
    let used_lot: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM stock_receipts WHERE run=?1 AND lot=?2)
         OR EXISTS(SELECT 1 FROM stock_lot_splits WHERE run=?1 AND child=?2)",
        params![run, lot],
        |row| row.get(0),
    )?;
    if used_lot {
        return Ok(failure("LOT_ALREADY_ASSIGNED"));
    }
    if tx.execute(
        "INSERT INTO stock_receipts (run,receipt,item,revision,unit,quantity,requires_inspection,lot,location,posted_cursor)
         VALUES (?1,?2,?3,?4,'each',?5,0,?6,?7,?8) ON CONFLICT(run,receipt) DO NOTHING",
        params![run,receipt,item["item_id"].as_str(),item["revision"].as_i64(),count as i64*QUANTITY_SCALE,lot,location,cursor],
    )? == 0 {
        return Ok(failure("RECEIPT_ALREADY_EXISTS"));
    }
    tx.execute("INSERT INTO work_order_output_receipts (run,work_order,receipt,serials) VALUES (?1,?2,?3,?4)",
        params![run,order,receipt,serde_json::to_string(serials)?])?;
    Ok(json!({"ok":true,"work_order":order,"output_receipt":read(tx,run,order)?}))
}

fn failure(code: &str) -> Value {
    json!({"ok":false,"code":code})
}
