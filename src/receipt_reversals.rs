use rusqlite::{OptionalExtension, Row, Transaction, params};
use serde_json::{Map, Value, json};

use crate::{
    catalog::{Spec, object, text},
    inventory::{id_schema, quantity, quantity_schema},
    scenario::{Rejection, reject},
};

pub(crate) fn row_schema() -> Value {
    object(
        &[
            "reversal_id",
            "receipt_id",
            "lot_id",
            "item_id",
            "revision",
            "location",
            "quantity",
            "unit",
            "reason",
            "reversed_cursor",
        ],
        json!({"reversal_id":id_schema(),"receipt_id":id_schema(),"lot_id":id_schema(),
            "item_id":id_schema(),"revision":{"type":"integer","minimum":1},"location":id_schema(),
            "quantity":quantity_schema(),"unit":text(),"reason":text(),"reversed_cursor":{"type":"integer","minimum":1}}),
    )
}

pub(crate) fn extend_spec(mut spec: Spec) -> Spec {
    spec.input["properties"]["movement_type"]["enum"] = json!(["transfer", "receipt"]);
    spec.output["oneOf"].as_array_mut().unwrap().push(object(
        &["ok", "run_id", "cursor", "receipt_reversal"],
        json!({"ok":{"const":true},"run_id":text(),"cursor":{"type":"integer","minimum":1},"receipt_reversal":row_schema()}),
    ));
    spec.errors = &[
        "INVALID_ARGUMENT",
        "LOT_NOT_FOUND",
        "TRANSFER_ALREADY_EXISTS",
        "TRANSFER_NOT_FOUND",
        "TRANSFER_SOURCE_CHANGED",
        "TRANSFER_NOT_POSTED",
        "TRANSFER_ALREADY_REVERSED",
        "TRANSFER_IS_REVERSAL",
        "TRANSFER_HAS_DEPENDENCIES",
        "LOT_PENDING_INSPECTION",
        "MATERIAL_HELD",
        "LOT_COMMITTED",
        "VERSION_CONFLICT",
        "IDEMPOTENCY_CONFLICT",
        "RECEIPT_NOT_FOUND",
        "RECEIPT_NOT_POSTED",
        "RECEIPT_ALREADY_REVERSED",
        "RECEIPT_HAS_DEPENDENCIES",
        "REVERSAL_ID_ALREADY_EXISTS",
    ];
    spec
}

const RECORDS: &str = "SELECT r.reversal,r.receipt,s.lot,s.item,s.revision,s.location,s.quantity,s.unit,r.reason,r.reversed_cursor
    FROM stock_receipt_reversals r JOIN stock_receipts s ON s.run=r.run AND s.receipt=r.receipt";

fn record(row: &Row<'_>) -> rusqlite::Result<Value> {
    Ok(
        json!({"reversal_id":row.get::<_,String>(0)?,"receipt_id":row.get::<_,String>(1)?,
        "lot_id":row.get::<_,String>(2)?,"item_id":row.get::<_,String>(3)?,"revision":row.get::<_,i64>(4)?,
        "location":row.get::<_,String>(5)?,"quantity":quantity(row.get(6)?),"unit":row.get::<_,String>(7)?,
        "reason":row.get::<_,String>(8)?,"reversed_cursor":row.get::<_,i64>(9)?}),
    )
}

pub(crate) fn list(
    tx: &Transaction<'_>,
    run: &str,
    args: &Map<String, Value>,
) -> rusqlite::Result<Value> {
    let after = args
        .get("after_movement_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let item = args.get("item_id").and_then(Value::as_str);
    let revision = args.get("revision").and_then(Value::as_i64);
    let location = args.get("location").and_then(Value::as_str);
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(20);
    let mut query = tx.prepare(&format!(
        "{RECORDS} WHERE r.run=?1 AND r.reversal>?2
        AND (?3 IS NULL OR s.item=?3) AND (?4 IS NULL OR s.revision=?4)
        AND (?5 IS NULL OR s.location=?5) ORDER BY r.reversal LIMIT ?6"
    ))?;
    let mut rows = query
        .query_map(
            params![run, after, item, revision, location, limit + 1],
            record,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let next = if rows.len() > limit as usize {
        rows.pop();
        rows.last().map(|row| row["reversal_id"].clone())
    } else {
        None
    };
    Ok(json!({"ok":true,"run_id":run,"movements":rows,"next_after_movement_id":next}))
}

pub(crate) fn reverse(
    tx: &Transaction<'_>,
    run: &str,
    id: &str,
    args: &Map<String, Value>,
    cursor: i64,
) -> rusqlite::Result<Result<Value, Rejection>> {
    let receipt = args["movement_id"].as_str().unwrap();
    let original = tx.query_row(
        "SELECT s.posted_cursor IS NOT NULL,
          EXISTS(SELECT 1 FROM stock_receipt_reversals r WHERE r.run=s.run AND r.receipt=s.receipt),
          EXISTS(SELECT 1 FROM scenario_holds h WHERE h.run=s.run AND h.batch=s.lot),
          EXISTS(SELECT 1 FROM stock_reservations r WHERE r.run=s.run AND r.lot=s.lot
            AND (r.status IN ('reserved','picked','issued') OR EXISTS(SELECT 1 FROM stock_operation_issues i
                WHERE i.run=r.run AND i.reservation=r.reservation)))
          OR EXISTS(SELECT 1 FROM stock_transfers t WHERE t.run=s.run AND t.lot=s.lot AND t.posted_cursor IS NOT NULL)
          OR EXISTS(SELECT 1 FROM stock_receipt_inspections i WHERE i.run=s.run AND i.receipt=s.receipt)
          OR EXISTS(SELECT 1 FROM stock_work_order_output o WHERE o.run=s.run AND o.receipt=s.receipt AND o.settled=1)
         FROM stock_receipts s WHERE s.run=?1 AND s.receipt=?2",
        params![run,receipt], |row| Ok((row.get::<_,bool>(0)?,row.get::<_,bool>(1)?,
            row.get::<_,bool>(2)?,row.get::<_,bool>(3)?)),
    ).optional()?;
    let Some((posted, reversed, held, dependent)) = original else {
        return Ok(Err(reject("RECEIPT_NOT_FOUND", "movement_id")));
    };
    if !posted {
        return Ok(Err(reject("RECEIPT_NOT_POSTED", "movement_id")));
    }
    for (blocked, code) in [
        (reversed, "RECEIPT_ALREADY_REVERSED"),
        (held, "MATERIAL_HELD"),
        (dependent, "RECEIPT_HAS_DEPENDENCIES"),
    ] {
        if blocked {
            return Ok(Err(reject(code, "movement_id")));
        }
    }
    if tx.execute(
        "INSERT INTO stock_receipt_reversals (run,receipt,reversal,reason,reversed_cursor)
         VALUES (?1,?2,?3,?4,?5) ON CONFLICT (run,reversal) DO NOTHING",
        params![
            run,
            receipt,
            id,
            args["reason"].as_str().unwrap(),
            cursor + 1
        ],
    )? == 0
    {
        return Ok(Err(reject("REVERSAL_ID_ALREADY_EXISTS", "reversal_id")));
    }
    let row = tx.query_row(
        &format!("{RECORDS} WHERE r.run=?1 AND r.reversal=?2"),
        params![run, id],
        record,
    )?;
    Ok(Ok(row))
}
