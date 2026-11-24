use std::collections::BTreeSet;

use rusqlite::{Transaction, params};
use serde_json::{Map, Value, json};

use crate::{
    catalog::{Action, Spec, object, text},
    inventory::{LOCATIONS, QUANTITY_SCALE, id_schema},
    mes::{QUANTITY, Report},
};

pub(crate) fn spec() -> Spec {
    Spec {
        name: Action::ReceiveWorkOrderOutput.name(),
        description: "按已放行的完工实绩登记成品批次及序列号",
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
        output: json!({"oneOf":[object(&["ok","cursor","receipt"],json!({
            "ok":{"const":true},"cursor":{"type":"integer","minimum":1},"receipt":crate::inventory::receipt_schema()})),
            {"type":"object","required":["ok","code"],"properties":{"ok":{"const":false},"code":text(),"cursor":{"type":"integer"}}}]}),
        preconditions: &[
            "run_open",
            "tool_enabled_in_scenario",
            "work_order_completed",
            "quality_released",
        ],
        side_effect: "post_finished_output_receipt",
        errors: &[
            "INVALID_ARGUMENT",
            "WORK_ORDER_NOT_FOUND",
            "COMPLETION_REQUIRED",
            "OUTPUT_NOT_RELEASED",
            "OUTPUT_RECEIPT_EXISTS",
            "OUTPUT_QUANTITY_INCOMPLETE",
            "ORDER_SNAPSHOT_NOT_FOUND",
            "OUTPUT_UNIT_UNSUPPORTED",
            "QUALITY_HOLD",
            "QUALITY_SCRAPPED",
            "MATERIAL_HELD",
            "LOCATION_NOT_FOUND",
            "RECEIPT_ALREADY_EXISTS",
            "LOT_ALREADY_ASSIGNED",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

const ACTIVE_RECEIPT: &str = "run=?1 AND work_order=?2 AND NOT EXISTS (
    SELECT 1 FROM stock_receipt_reversals r
    WHERE r.run=stock_work_order_output.run AND r.receipt=stock_work_order_output.receipt)";

pub(crate) fn active(tx: &Transaction<'_>, run: &str, order: &str) -> rusqlite::Result<bool> {
    tx.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM stock_work_order_output WHERE {ACTIVE_RECEIPT})"),
        params![run, order],
        |row| row.get(0),
    )
}

pub(crate) fn settle(tx: &Transaction<'_>, run: &str, order: &str) -> rusqlite::Result<bool> {
    Ok(tx.execute(
        &format!("UPDATE stock_work_order_output SET settled=1 WHERE {ACTIVE_RECEIPT}"),
        params![run, order],
    )? == 1)
}

pub(crate) fn receive(
    tx: &Transaction<'_>,
    run: &str,
    reports: &[Report],
    args: &Map<String, Value>,
    cursor: i64,
) -> anyhow::Result<Value> {
    let order = args["work_order"].as_str().unwrap();
    let receipt = args["receipt_id"].as_str().unwrap();
    let lot = args["lot_id"].as_str().unwrap();
    let location = args["location"].as_str().unwrap();
    let fail = |code| Ok(json!({"ok":false,"code":code}));
    if !LOCATIONS.contains(&location) {
        return fail("LOCATION_NOT_FOUND");
    }
    if active(tx, run, order)? {
        return fail("OUTPUT_RECEIPT_EXISTS");
    }
    let mut serials = BTreeSet::new();
    let mut quantity = 0;
    for report in reports
        .iter()
        .filter(|report| report.order == order && !report.reversed)
    {
        if report.step == 0 && crate::wip::material_held(tx, run, report)? {
            return fail("MATERIAL_HELD");
        }
        if report.step == 1 {
            quantity += report.quantity;
            serials.extend(report.serial_numbers.iter());
        }
    }
    if quantity != QUANTITY || serials.len() != quantity {
        return fail("OUTPUT_QUANTITY_INCOMPLETE");
    }
    let Some(snapshot) = crate::snapshots::load(tx, run, order)? else {
        return fail("ORDER_SNAPSHOT_NOT_FOUND");
    };
    let item = &snapshot["item"];
    if item["base_unit"] != "each" {
        return fail("OUTPUT_UNIT_UNSUPPORTED");
    }
    let exists = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM stock_receipts WHERE run=?1 AND receipt=?2),
         EXISTS(SELECT 1 FROM stock_receipts WHERE run=?1 AND lot=?3)
            OR EXISTS(SELECT 1 FROM stock_lot_splits WHERE run=?1 AND child=?3)",
        params![run, receipt, lot],
        |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
    )?;
    if exists.0 {
        return fail("RECEIPT_ALREADY_EXISTS");
    }
    if exists.1 {
        return fail("LOT_ALREADY_ASSIGNED");
    }
    tx.execute(
        "INSERT INTO stock_receipts (run,receipt,item,revision,unit,quantity,requires_inspection,lot,location,posted_cursor)
         VALUES (?1,?2,?3,?4,'each',?5,0,?6,?7,?8)",
        params![run,receipt,item["item_id"].as_str(),item["revision"].as_i64(),
            quantity as i64 * QUANTITY_SCALE,lot,location,cursor],
    )?;
    tx.execute(
        "INSERT INTO stock_work_order_output (run,receipt,work_order,serial_numbers)
         VALUES (?1,?2,?3,?4)",
        params![run, receipt, order, serde_json::to_string(&serials)?],
    )?;
    Ok(json!({"ok":true,"receipt":crate::inventory::read(tx,run,receipt)?}))
}

pub(crate) struct OutputReceipt {
    pub(crate) cursor: i64,
    pub(crate) order: String,
    pub(crate) lot: String,
    pub(crate) serials: Vec<String>,
    pub(crate) reversed: bool,
}

pub(crate) fn genealogy(
    tx: &Transaction<'_>,
    run: &str,
    order: Option<&str>,
) -> anyhow::Result<Vec<OutputReceipt>> {
    let mut statement = tx.prepare(
        "SELECT s.posted_cursor,o.work_order,s.lot,o.serial_numbers,r.receipt IS NOT NULL
         FROM stock_work_order_output o
         JOIN stock_receipts s ON s.run=o.run AND s.receipt=o.receipt
         LEFT JOIN stock_receipt_reversals r ON r.run=o.run AND r.receipt=o.receipt
         WHERE o.run=?1 AND (?2 IS NULL OR o.work_order=?2)",
    )?;
    let receipts = statement.query_map(params![run, order], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get::<_, String>(3)?,
            row.get(4)?,
        ))
    })?;
    receipts
        .map(|row| {
            let (cursor, order, lot, serials, reversed) = row?;
            Ok(OutputReceipt {
                cursor,
                order,
                lot,
                serials: serde_json::from_str(&serials)?,
                reversed,
            })
        })
        .collect()
}
