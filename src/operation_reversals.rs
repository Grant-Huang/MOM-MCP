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
            "original_movement_id",
            "movement_type",
            "reservation_id",
            "lot_id",
            "item_id",
            "revision",
            "location",
            "quantity",
            "unit",
            "reason",
            "reversed_cursor",
        ],
        json!({"reversal_id":id_schema(),"original_movement_id":id_schema(),"movement_type":{"enum":["issue","return"]},
            "reservation_id":id_schema(),"lot_id":id_schema(),"item_id":id_schema(),"revision":{"type":"integer","minimum":1},
            "location":id_schema(),"quantity":quantity_schema(),"unit":text(),"reason":text(),"reversed_cursor":{"type":"integer","minimum":1}}),
    )
}

pub(crate) fn extend_spec(mut spec: Spec) -> Spec {
    spec.input["properties"]["movement_type"]["enum"] =
        json!(["transfer", "receipt", "issue", "return"]);
    spec.output["oneOf"].as_array_mut().unwrap().push(object(
        &["ok","run_id","cursor","operation_reversal"],
        json!({"ok":{"const":true},"run_id":text(),"cursor":{"type":"integer","minimum":1},"operation_reversal":row_schema()}),
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
        "MOVEMENT_NOT_FOUND",
        "MOVEMENT_ALREADY_REVERSED",
        "MOVEMENT_HAS_DEPENDENCIES",
        "LOT_AVAILABLE_QUANTITY_EXCEEDED",
        "ORDER_REQUIREMENT_EXCEEDED",
        "ORDER_SNAPSHOT_NOT_FOUND",
        "BOM_QUANTITY_UNSUPPORTED",
        "BOM_EXPANSION_LIMIT",
    ];
    spec
}

const RECORDS: &str = "SELECT r.reversal,r.original,r.kind,r.reservation,r.lot,s.item,s.revision,
    r.location,r.quantity,s.unit,r.reason,r.reversed_cursor
    FROM stock_operation_reversals r JOIN stock_lot_origins s ON s.run=r.run AND s.lot=r.lot";

fn record(row: &Row<'_>) -> rusqlite::Result<Value> {
    Ok(
        json!({"reversal_id":row.get::<_,String>(0)?,"original_movement_id":row.get::<_,String>(1)?,
        "movement_type":row.get::<_,String>(2)?,"reservation_id":row.get::<_,String>(3)?,"lot_id":row.get::<_,String>(4)?,
        "item_id":row.get::<_,String>(5)?,"revision":row.get::<_,i64>(6)?,"location":row.get::<_,String>(7)?,
        "quantity":quantity(row.get(8)?),"unit":row.get::<_,String>(9)?,"reason":row.get::<_,String>(10)?,
        "reversed_cursor":row.get::<_,i64>(11)?}),
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
        AND (?5 IS NULL OR r.location=?5) ORDER BY r.reversal LIMIT ?6"
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

struct Target {
    reservation: String,
    order: String,
    lot: String,
    quantity: i64,
    capacity: i64,
    effective_returns: i64,
    item: String,
    revision: i64,
    unit: String,
    location: String,
    held: bool,
    moved: bool,
}

fn target(
    tx: &Transaction<'_>,
    run: &str,
    original: &str,
    is_issue: bool,
) -> rusqlite::Result<Option<Target>> {
    tx.query_row(
        "SELECT r.reservation,r.work_order,r.lot,CASE WHEN ?3 THEN r.quantity ELSE t.quantity END,
            s.quantity,COALESCE(e.quantity,0),s.item,s.revision,s.unit,s.location,
            EXISTS(SELECT 1 FROM stock_lot_holds h WHERE h.run=r.run AND h.batch=r.lot),
            EXISTS(SELECT 1 FROM stock_transfers m WHERE m.run=r.run AND m.lot=r.lot
                AND m.posted_cursor>CASE WHEN ?3 THEN i.issued_cursor ELSE t.returned_cursor END)
         FROM stock_reservations r
         JOIN stock_operation_issues i ON i.run=r.run AND i.reservation=r.reservation
         JOIN stock_available_lots s ON s.run=r.run AND s.lot=r.lot
         LEFT JOIN stock_operation_returns t ON NOT ?3 AND t.run=r.run AND t.reservation=r.reservation AND t.movement=?2
         LEFT JOIN stock_effective_return_totals e ON e.run=r.run AND e.reservation=r.reservation
         WHERE r.run=?1 AND r.status='issued' AND ((?3 AND r.reservation=?2) OR t.movement=?2)",
        params![run,original,is_issue], |row| Ok(Target {
            reservation:row.get(0)?,order:row.get(1)?,lot:row.get(2)?,quantity:row.get(3)?,capacity:row.get(4)?,
            effective_returns:row.get(5)?,item:row.get(6)?,revision:row.get(7)?,unit:row.get(8)?,
            location:row.get(9)?,held:row.get(10)?,moved:row.get(11)?,
        }),
    ).optional()
}

pub(crate) fn reverse(
    tx: &Transaction<'_>,
    run: &str,
    id: &str,
    args: &Map<String, Value>,
    cursor: i64,
    order_quantity: i64,
) -> anyhow::Result<Result<Value, Rejection>> {
    let original = args["movement_id"].as_str().unwrap();
    let kind = args["movement_type"].as_str().unwrap();
    let is_issue = kind == "issue";
    let reversed: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM stock_operation_reversals WHERE run=?1 AND kind=?2 AND original=?3)",
        params![run,kind,original], |row| row.get(0),
    )?;
    if reversed {
        return Ok(Err(reject("MOVEMENT_ALREADY_REVERSED", "movement_id")));
    }
    let Some(target) = target(tx, run, original, is_issue)? else {
        return Ok(Err(reject("MOVEMENT_NOT_FOUND", "movement_id")));
    };
    if target.held {
        return Ok(Err(reject("MATERIAL_HELD", "movement_id")));
    }
    if target.moved {
        return Ok(Err(reject("MOVEMENT_HAS_DEPENDENCIES", "movement_id")));
    }
    if is_issue {
        let used =
            crate::mes::reported_material_quantity(tx, run, &target.order, &target.reservation)?;
        if used != 0 || target.effective_returns != 0 {
            return Ok(Err(reject("MOVEMENT_HAS_DEPENDENCIES", "movement_id")));
        }
    } else {
        if target.quantity
            > target.capacity - crate::reservations::lot_committed_quantity(tx, run, &target.lot)?
        {
            return Ok(Err(reject(
                "LOT_AVAILABLE_QUANTITY_EXCEEDED",
                "movement_id",
            )));
        }
        let required = match crate::inventory::required_quantity(
            tx,
            run,
            &target.order,
            order_quantity,
            &target.item,
            target.revision,
            &target.unit,
        )? {
            Ok(quantity) => quantity,
            Err(error) => return Ok(Err(error)),
        };
        let reserved = crate::reservations::order_quantity_by_state(
            tx,
            run,
            &target.order,
            &target.item,
            target.revision,
            false,
        )?;
        let issued = crate::reservations::order_quantity_by_state(
            tx,
            run,
            &target.order,
            &target.item,
            target.revision,
            true,
        )?;
        if target.quantity > required - reserved - issued {
            return Ok(Err(reject("ORDER_REQUIREMENT_EXCEEDED", "movement_id")));
        }
    }
    if tx.execute(
        "INSERT INTO stock_operation_reversals (run,kind,original,reversal,reservation,lot,location,quantity,reason,reversed_cursor)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT (run,reversal) DO NOTHING",
        params![run,kind,original,id,target.reservation,target.lot,target.location,target.quantity,args["reason"].as_str().unwrap(),cursor+1],
    )? == 0 {
        return Ok(Err(reject("REVERSAL_ID_ALREADY_EXISTS","reversal_id")));
    }
    if is_issue {
        tx.execute(
            "UPDATE stock_reservations SET status='released' WHERE run=?1 AND reservation=?2",
            params![run, target.reservation],
        )?;
    }
    let row = tx.query_row(
        &format!("{RECORDS} WHERE r.run=?1 AND r.reversal=?2"),
        params![run, id],
        record,
    )?;
    Ok(Ok(row))
}
