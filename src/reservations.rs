use rusqlite::{OptionalExtension, Row, Transaction, params};
use serde_json::{Map, Value, json};

use crate::{
    catalog::{Action, Spec, object, text},
    inventory::{QUANTITY_SCALE, id_schema, parse_quantity, quantity, quantity_schema},
    scenario::{Rejection, identifier, reject},
};

pub(crate) fn row_schema() -> Value {
    object(
        &[
            "reservation_id",
            "work_order",
            "lot_id",
            "item_id",
            "revision",
            "location",
            "quantity",
            "unit",
            "status",
        ],
        json!({
        "reservation_id":id_schema(),"work_order":id_schema(),"lot_id":id_schema(),"item_id":id_schema(),
        "revision":{"type":"integer","minimum":1},"location":id_schema(),"quantity":quantity_schema(),"unit":text(),
        "status":{"enum":["reserved","picked","released","issued"]},"operation":id_schema(),"work_center":id_schema(),"issued_cursor":{"type":"integer","minimum":1}}),
    )
}

pub(crate) fn spec(action: Action) -> Spec {
    let mut fields = json!({"reservation_id":id_schema(),
        "expected_cursor":{"type":"integer","minimum":0,"maximum":i64::MAX},"idempotency_key":text()});
    let mut required = vec!["reservation_id", "expected_cursor", "idempotency_key"];
    let read = action == Action::ListReservations;
    let description = match action {
        Action::ReserveMaterial => {
            required.extend(["work_order", "lot_id", "quantity"]);
            fields["work_order"] = id_schema();
            fields["lot_id"] = id_schema();
            fields["quantity"] = quantity_schema();
            "按冻结工单需求预留指定批次"
        }
        Action::ListReservations => {
            required.clear();
            fields = json!({"work_order":id_schema(),"lot_id":id_schema(),"after_reservation_id":id_schema(),
                "limit":{"type":"integer","minimum":1,"maximum":100}});
            "分页查询工单或批次预留"
        }
        Action::IssueMaterialToOperation => {
            required.push("operation");
            fields["operation"] = id_schema();
            "将已拣料批次发至指定工序"
        }
        Action::ReturnUnusedMaterial => {
            required.extend(["movement_id", "quantity", "reason"]);
            fields["movement_id"] = id_schema();
            fields["quantity"] = quantity_schema();
            fields["reason"] = text();
            "将工序尚未消耗的物料退回发料批次及原库位"
        }
        Action::PickMaterial => "将已预留的物料标记为拣料",
        Action::UnpickMaterial => "撤销拣料并恢复预留",
        Action::ReleaseReservation => "释放尚未拣料的预留",
        _ => unreachable!("reservation action"),
    };
    let output = if read {
        object(
            &["ok", "run_id", "reservations", "next_after_reservation_id"],
            json!({"ok":{"const":true},
            "run_id":text(),"reservations":{"type":"array","items":row_schema()},
            "next_after_reservation_id":{"type":["string","null"]}}),
        )
    } else if action == Action::ReturnUnusedMaterial {
        object(
            &["ok", "run_id", "cursor", "movement"],
            json!({"ok":{"const":true},"run_id":text(),"cursor":{"type":"integer","minimum":1},"movement":return_schema()}),
        )
    } else {
        object(
            &["ok", "run_id", "cursor", "reservation"],
            json!({"ok":{"const":true},
            "run_id":text(),"cursor":{"type":"integer","minimum":1},"reservation":row_schema()}),
        )
    };
    Spec {
        name: action.name(),
        description,
        domain: "inventory",
        effect: if read { "read" } else { "write" },
        input: object(&required, fields),
        output: json!({"oneOf":[output,object(&["ok","code","field"],
            json!({"ok":{"const":false},"code":text(),"field":text()}))]}),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: if read { "none" } else { action.name() },
        errors: &[
            "INVALID_ARGUMENT",
            "WORK_ORDER_NOT_RESERVABLE",
            "ORDER_SNAPSHOT_NOT_FOUND",
            "BOM_QUANTITY_UNSUPPORTED",
            "BOM_EXPANSION_LIMIT",
            "LOT_NOT_FOUND",
            "LOT_PENDING_INSPECTION",
            "LOT_AVAILABLE_QUANTITY_EXCEEDED",
            "MATERIAL_NOT_REQUIRED",
            "ORDER_REQUIREMENT_EXCEEDED",
            "RESERVATION_ALREADY_EXISTS",
            "RESERVATION_NOT_FOUND",
            "RESERVATION_STATE_INVALID",
            "ISSUE_OPERATION_NOT_FOUND",
            "ISSUE_OPERATION_NOT_ELIGIBLE",
            "QUALITY_HOLD",
            "QUALITY_SCRAPPED",
            "RETURN_MOVEMENT_ALREADY_EXISTS",
            "MATERIAL_HELD",
            "RETURN_EXCEEDS_UNUSED_QUANTITY",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

const RECORDS: &str = "SELECT r.reservation,r.work_order,r.lot,r.quantity,r.status,
    s.item,s.revision,s.unit,CASE WHEN i.issued_cursor IS NULL THEN c.location ELSE COALESCE((
        SELECT t.destination FROM stock_transfers t WHERE t.run=r.run AND t.lot=r.lot
        AND t.posted_cursor<i.issued_cursor ORDER BY t.posted_cursor DESC LIMIT 1),s.location) END AS issue_location,
    i.operation,i.work_center,i.issued_cursor FROM stock_reservations r
    JOIN stock_receipts s ON s.run=r.run AND s.lot=r.lot
    JOIN stock_current_lots c ON c.run=r.run AND c.lot=r.lot
    LEFT JOIN stock_operation_issues i ON i.run=r.run AND i.reservation=r.reservation";

fn record(row: &Row<'_>) -> rusqlite::Result<Value> {
    let mut result = json!({"reservation_id":row.get::<_,String>(0)?,"work_order":row.get::<_,String>(1)?,
        "lot_id":row.get::<_,String>(2)?,"quantity":quantity(row.get(3)?),"status":row.get::<_,String>(4)?,
        "item_id":row.get::<_,String>(5)?,"revision":row.get::<_,i64>(6)?,"unit":row.get::<_,String>(7)?,
        "location":row.get::<_,String>(8)?});
    if let Some(operation) = row.get::<_, Option<String>>(9)? {
        result["operation"] = json!(operation);
        result["work_center"] = json!(row.get::<_, String>(10)?);
        result["issued_cursor"] = json!(row.get::<_, i64>(11)?);
    }
    Ok(result)
}

pub(crate) fn issue_movements(
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
    let mut query=tx.prepare(&format!("{RECORDS} WHERE r.run=?1 AND r.reservation>?2
        AND i.reservation IS NOT NULL AND (?3 IS NULL OR s.item=?3) AND (?4 IS NULL OR s.revision=?4)
        AND (?5 IS NULL OR issue_location=?5) ORDER BY r.reservation LIMIT ?6"))?;
    let mut rows = query
        .query_map(
            params![run, after, item, revision, location, limit + 1],
            record,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let next = if rows.len() > limit as usize {
        rows.pop();
        rows.last().map(|row| row["reservation_id"].clone())
    } else {
        None
    };
    Ok(json!({"ok":true,"run_id":run,"movements":rows,"next_after_movement_id":next}))
}

pub(crate) fn return_schema() -> Value {
    object(
        &[
            "movement_id",
            "reservation_id",
            "work_order",
            "operation",
            "lot_id",
            "item_id",
            "revision",
            "location",
            "quantity",
            "unit",
            "reason",
            "returned_cursor",
        ],
        json!({
            "movement_id":id_schema(),"reservation_id":id_schema(),"work_order":id_schema(),
            "operation":id_schema(),"lot_id":id_schema(),"item_id":id_schema(),"revision":{"type":"integer","minimum":1},
            "location":id_schema(),"quantity":quantity_schema(),"unit":text(),"reason":text(),
            "returned_cursor":{"type":"integer","minimum":1}
        }),
    )
}

const RETURN_RECORDS: &str = "SELECT t.movement,t.reservation,r.work_order,i.operation,r.lot,
    s.item,s.revision,COALESCE((SELECT m.destination FROM stock_transfers m WHERE m.run=r.run AND m.lot=r.lot
        AND m.posted_cursor<i.issued_cursor ORDER BY m.posted_cursor DESC LIMIT 1),s.location) AS issue_location,
    t.quantity,s.unit,t.reason,t.returned_cursor
    FROM stock_operation_returns t
    JOIN stock_reservations r ON r.run=t.run AND r.reservation=t.reservation
    JOIN stock_operation_issues i ON i.run=r.run AND i.reservation=r.reservation
    JOIN stock_receipts s ON s.run=r.run AND s.lot=r.lot";

fn return_record(row: &Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "movement_id":row.get::<_,String>(0)?,"reservation_id":row.get::<_,String>(1)?,
        "work_order":row.get::<_,String>(2)?,"operation":row.get::<_,String>(3)?,"lot_id":row.get::<_,String>(4)?,
        "item_id":row.get::<_,String>(5)?,"revision":row.get::<_,i64>(6)?,"location":row.get::<_,String>(7)?,
        "quantity":quantity(row.get(8)?),"unit":row.get::<_,String>(9)?,"reason":row.get::<_,String>(10)?,
        "returned_cursor":row.get::<_,i64>(11)?
    }))
}

pub(crate) fn return_movements(
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
        "{RETURN_RECORDS} WHERE t.run=?1 AND t.movement>?2
        AND (?3 IS NULL OR s.item=?3) AND (?4 IS NULL OR s.revision=?4)
        AND (?5 IS NULL OR issue_location=?5) ORDER BY t.movement LIMIT ?6"
    ))?;
    let mut rows = query
        .query_map(
            params![run, after, item, revision, location, limit + 1],
            return_record,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let next = if rows.len() > limit as usize {
        rows.pop();
        rows.last().map(|row| row["movement_id"].clone())
    } else {
        None
    };
    Ok(json!({"ok":true,"run_id":run,"movements":rows,"next_after_movement_id":next}))
}

fn return_unused(
    tx: &Transaction<'_>,
    run: &str,
    id: &str,
    args: &Map<String, Value>,
    cursor: i64,
) -> anyhow::Result<Result<Value, Rejection>> {
    let issue = tx
        .query_row(
            "SELECT r.work_order,s.unit,r.quantity-COALESCE(t.quantity,0),
            EXISTS(SELECT 1 FROM scenario_holds h WHERE h.run=r.run AND h.batch=r.lot)
         FROM stock_operation_issues i
         JOIN stock_reservations r ON r.run=i.run AND r.reservation=i.reservation
         JOIN stock_receipts s ON s.run=r.run AND s.lot=r.lot
         LEFT JOIN stock_return_totals t ON t.run=r.run AND t.reservation=r.reservation
         WHERE i.run=?1 AND i.reservation=?2 AND r.status='issued'",
            params![run, id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((order, unit, remaining, held)) = issue else {
        let code = if read(tx, run, id)?.is_some() {
            "RESERVATION_STATE_INVALID"
        } else {
            "RESERVATION_NOT_FOUND"
        };
        return Ok(Err(reject(code, "reservation_id")));
    };
    if held {
        return Ok(Err(reject("MATERIAL_HELD", "reservation_id")));
    }
    let Some(amount) = parse_quantity(args["quantity"].as_str().unwrap())
        .filter(|amount| unit != "each" || amount % QUANTITY_SCALE == 0)
    else {
        return Ok(Err(Rejection::invalid("quantity")));
    };
    let movement = args["movement_id"].as_str().unwrap();
    let duplicate: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM stock_operation_returns WHERE run=?1 AND movement=?2)",
        params![run, movement],
        |row| row.get(0),
    )?;
    if duplicate {
        return Ok(Err(reject("RETURN_MOVEMENT_ALREADY_EXISTS", "movement_id")));
    }
    let used = crate::mes::reported_material_quantity(tx, run, &order, id)?;
    if amount > remaining - used {
        return Ok(Err(reject("RETURN_EXCEEDS_UNUSED_QUANTITY", "quantity")));
    }
    tx.execute(
        "INSERT INTO stock_operation_returns (run,movement,reservation,quantity,reason,returned_cursor)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![run,movement,id,amount,args["reason"].as_str().unwrap(),cursor+1],
    )?;
    let row = tx.query_row(
        &format!("{RETURN_RECORDS} WHERE t.run=?1 AND t.movement=?2"),
        params![run, movement],
        return_record,
    )?;
    Ok(Ok(
        json!({"ok":true,"run_id":run,"cursor":cursor+1,"movement":row}),
    ))
}

fn read(tx: &Transaction<'_>, run: &str, id: &str) -> rusqlite::Result<Option<Value>> {
    tx.query_row(
        &format!("{RECORDS} WHERE r.run=?1 AND r.reservation=?2"),
        params![run, id],
        record,
    )
    .optional()
}

pub(crate) fn order_quantity_by_state(
    tx: &Transaction<'_>,
    run: &str,
    order: &str,
    item: &str,
    revision: i64,
    issued: bool,
) -> rusqlite::Result<i64> {
    tx.query_row(
        "SELECT COALESCE(SUM(r.quantity-COALESCE(t.quantity,0)),0) FROM stock_reservations r
         JOIN stock_receipts s ON s.run=r.run AND s.lot=r.lot
         LEFT JOIN stock_return_totals t ON t.run=r.run AND t.reservation=r.reservation
         WHERE r.run=?1 AND r.work_order=?2 AND s.item=?3 AND s.revision=?4
         AND ((?5 AND r.status='issued') OR (NOT ?5 AND r.status IN ('reserved','picked'))) ",
        params![run, order, item, revision, issued],
        |row| row.get(0),
    )
}

pub(crate) fn invoke(
    tx: Transaction<'_>,
    run: &str,
    action: Action,
    args: &Map<String, Value>,
    cursor: i64,
    order_quantity: i64,
    eligible: bool,
    issue: Result<Option<String>, &'static str>,
) -> anyhow::Result<Result<Value, Rejection>> {
    for field in [
        "reservation_id",
        "movement_id",
        "work_order",
        "lot_id",
        "after_reservation_id",
        "operation",
    ] {
        if args
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|id| !identifier(id))
        {
            return Ok(Err(Rejection::invalid(field)));
        }
    }
    if action == Action::ListReservations {
        let after = args
            .get("after_reservation_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let work_order = args.get("work_order").and_then(Value::as_str);
        let lot = args.get("lot_id").and_then(Value::as_str);
        let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(20);
        let mut query = tx.prepare(&format!(
            "{RECORDS} WHERE r.run=?1 AND r.reservation>?2
            AND (?3 IS NULL OR r.work_order=?3) AND (?4 IS NULL OR r.lot=?4)
            ORDER BY r.reservation LIMIT ?5"
        ))?;
        let mut rows = query
            .query_map(params![run, after, work_order, lot, limit + 1], record)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next = if rows.len() > limit as usize {
            rows.pop();
            rows.last().map(|row| row["reservation_id"].clone())
        } else {
            None
        };
        return Ok(Ok(
            json!({"ok":true,"run_id":run,"reservations":rows,"next_after_reservation_id":next}),
        ));
    }

    let key = args["idempotency_key"].as_str().unwrap();
    let request = json!({"action":action,"arguments":args}).to_string();
    let saved = tx
        .query_row(
            "SELECT request,response FROM scenario_requests WHERE run=?1 AND key=?2",
            params![run, key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if let Some((original, response)) = saved {
        return Ok(if original == request {
            Ok(serde_json::from_str(&response)?)
        } else {
            Err(reject("IDEMPOTENCY_CONFLICT", "idempotency_key"))
        });
    }
    if args["expected_cursor"].as_i64() != Some(cursor) {
        return Ok(Err(reject("VERSION_CONFLICT", "expected_cursor")));
    }
    let id = args["reservation_id"].as_str().unwrap();
    if action == Action::ReturnUnusedMaterial {
        return match return_unused(&tx, run, id, args, cursor)? {
            Ok(response) => finish_write(tx, run, key, &request, response),
            Err(error) => Ok(Err(error)),
        };
    }
    if action == Action::ReserveMaterial {
        if !eligible {
            return Ok(Err(reject("WORK_ORDER_NOT_RESERVABLE", "work_order")));
        }
        if read(&tx, run, id)?.is_some() {
            return Ok(Err(reject("RESERVATION_ALREADY_EXISTS", "reservation_id")));
        }
        let lot = args["lot_id"].as_str().unwrap();
        let order = args["work_order"].as_str().unwrap();
        let stock=tx.query_row(
        "SELECT s.item,s.revision,s.unit,s.quantity,s.requires_inspection AND q.receipt IS NULL
         FROM stock_active_lots s LEFT JOIN stock_receipt_releases q ON q.run=s.run AND q.receipt=s.receipt
         WHERE s.run=?1 AND s.lot=?2 AND s.posted_cursor IS NOT NULL",params![run,lot],
        |r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,
                r.get::<_,i64>(3)?,r.get::<_,bool>(4)?))).optional()?;
        let Some((item, revision, unit, on_hand, pending)) = stock else {
            return Ok(Err(reject("LOT_NOT_FOUND", "lot_id")));
        };
        if pending {
            return Ok(Err(reject("LOT_PENDING_INSPECTION", "lot_id")));
        }
        let Some(amount) = parse_quantity(args["quantity"].as_str().unwrap())
            .filter(|amount| unit != "each" || amount % QUANTITY_SCALE == 0)
        else {
            return Ok(Err(Rejection::invalid("quantity")));
        };
        let required = match crate::inventory::required_quantity(
            &tx,
            run,
            order,
            order_quantity,
            &item,
            revision,
            &unit,
        )? {
            Ok(quantity) => quantity,
            Err(error) => return Ok(Err(error)),
        };
        if required == 0 {
            return Ok(Err(reject("MATERIAL_NOT_REQUIRED", "lot_id")));
        }
        let reserved = order_quantity_by_state(&tx, run, order, &item, revision, false)?;
        let issued = order_quantity_by_state(&tx, run, order, &item, revision, true)?;
        if amount > required - reserved - issued {
            return Ok(Err(reject("ORDER_REQUIREMENT_EXCEEDED", "quantity")));
        }
        let held: i64 = tx.query_row(
            "SELECT COALESCE(SUM(r.quantity-COALESCE(t.quantity,0)),0) FROM stock_reservations r
        LEFT JOIN stock_return_totals t ON t.run=r.run AND t.reservation=r.reservation
        WHERE r.run=?1 AND r.lot=?2 AND r.status IN ('reserved','picked','issued')",
            params![run, lot],
            |r| r.get(0),
        )?;
        if amount > on_hand - held {
            return Ok(Err(reject("LOT_AVAILABLE_QUANTITY_EXCEEDED", "quantity")));
        }
        tx.execute(
            "INSERT INTO stock_reservations (run,reservation,work_order,lot,quantity,status)
        VALUES (?1,?2,?3,?4,?5,'reserved')",
            params![run, id, order, lot, amount],
        )?;
    } else {
        let Some(row) = read(&tx, run, id)? else {
            return Ok(Err(reject("RESERVATION_NOT_FOUND", "reservation_id")));
        };
        if action == Action::PickMaterial && !eligible {
            return Ok(Err(reject("WORK_ORDER_NOT_RESERVABLE", "work_order")));
        }
        let status = match (action, row["status"].as_str()) {
            (Action::IssueMaterialToOperation, Some("picked")) => {
                let center = match issue {
                    Ok(Some(center)) if eligible => center,
                    Err(code) => return Ok(Err(reject(code, "operation"))),
                    _ => return Ok(Err(reject("ISSUE_OPERATION_NOT_ELIGIBLE", "operation"))),
                };
                tx.execute("INSERT INTO stock_operation_issues (run,reservation,operation,work_center,issued_cursor)
                VALUES (?1,?2,?3,?4,?5)",params![run,id,args["operation"].as_str().unwrap(),center,cursor+1])?;
                "issued"
            }
            (Action::PickMaterial, Some("reserved")) => "picked",
            (Action::UnpickMaterial, Some("picked")) => "reserved",
            (Action::ReleaseReservation, Some("reserved")) => "released",
            _ => return Ok(Err(reject("RESERVATION_STATE_INVALID", "reservation_id"))),
        };
        tx.execute(
            "UPDATE stock_reservations SET status=?3 WHERE run=?1 AND reservation=?2",
            params![run, id, status],
        )?;
    }
    let response =
        json!({"ok":true,"run_id":run,"cursor":cursor+1,"reservation":read(&tx,run,id)?});
    finish_write(tx, run, key, &request, response)
}

pub(crate) fn finish_write(
    tx: Transaction<'_>,
    run: &str,
    key: &str,
    request: &str,
    response: Value,
) -> anyhow::Result<Result<Value, Rejection>> {
    tx.execute(
        "UPDATE scenario_runs SET cursor=cursor+1 WHERE id=?1",
        [run],
    )?;
    tx.execute(
        "INSERT INTO scenario_requests VALUES (?1,?2,?3,?4)",
        params![run, key, request, response.to_string()],
    )?;
    tx.commit()?;
    Ok(Ok(response))
}
