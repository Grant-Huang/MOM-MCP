use rusqlite::{OptionalExtension, Row, Transaction, params};
use serde_json::{Map, Value, json};

use crate::{
    catalog::{Action, Spec, object, text},
    inventory::{LOCATIONS, QUANTITY_SCALE, id_schema, parse_quantity, quantity, quantity_schema},
    scenario::{Rejection, identifier, reject},
};

pub(crate) fn row_schema() -> Value {
    object(
        &[
            "transfer_id",
            "lot_id",
            "item_id",
            "revision",
            "unit",
            "source_location",
            "destination_location",
            "reason",
            "status",
            "quantity",
            "posted_cursor",
        ],
        json!({"transfer_id":id_schema(),"lot_id":id_schema(),"item_id":id_schema(),
            "revision":{"type":"integer","minimum":1},"unit":text(),"source_location":id_schema(),
            "destination_location":id_schema(),"reason":text(),"status":{"enum":["draft","posted"]},
            "quantity":{"anyOf":[quantity_schema(),{"type":"null"}]},
            "posted_cursor":{"type":["integer","null"],"minimum":1},"reverses_transfer_id":id_schema(),"child_lot_id":id_schema(),"requested_quantity":quantity_schema()}),
    )
}

pub(crate) fn spec(action: Action) -> Spec {
    let mut fields = json!({"expected_cursor":{"type":"integer","minimum":0,"maximum":i64::MAX},"idempotency_key":text()});
    let mut required = vec!["expected_cursor", "idempotency_key"];
    let description = match action {
        Action::CreateStockTransfer => {
            required.extend([
                "transfer_id",
                "lot_id",
                "source_location",
                "destination_location",
                "reason",
            ]);
            for field in [
                "transfer_id",
                "lot_id",
                "source_location",
                "destination_location",
            ] {
                fields[field] = id_schema();
            }
            fields["reason"] = text();
            "创建整批库存库位转储单"
        }
        Action::PostStockTransfer => {
            required.push("transfer_id");
            fields["transfer_id"] = id_schema();
            "核对整批库存可用性并过账库位转储"
        }
        Action::ReverseInventoryMovement => {
            required.extend(["movement_type", "movement_id", "reversal_id", "reason"]);
            fields["movement_type"] = json!({"type":"string","enum":["transfer"]});
            fields["movement_id"] = id_schema();
            fields["reversal_id"] = id_schema();
            fields["reason"] = text();
            "冲销已过账库存流水并记录反向流水"
        }
        _ => unreachable!("stock transfer action"),
    };
    Spec {
        name: action.name(),
        description,
        domain: "inventory",
        effect: "write",
        input: object(&required, fields),
        output: json!({"oneOf":[object(&["ok","run_id","cursor","transfer"],
            json!({"ok":{"const":true},"run_id":text(),"cursor":{"type":"integer","minimum":1},"transfer":row_schema()})),
            object(&["ok","code","field"],json!({"ok":{"const":false},"code":text(),"field":text()}))]}),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: action.name(),
        errors: &[
            "INVALID_ARGUMENT",
            "LOCATION_NOT_FOUND",
            "LOT_NOT_FOUND",
            "TRANSFER_ALREADY_EXISTS",
            "TRANSFER_NOT_FOUND",
            "TRANSFER_ALREADY_POSTED",
            "TRANSFER_SOURCE_CHANGED",
            "TRANSFER_NOT_POSTED",
            "TRANSFER_ALREADY_REVERSED",
            "TRANSFER_IS_REVERSAL",
            "TRANSFER_HAS_DEPENDENCIES",
            "LOT_PENDING_INSPECTION",
            "MATERIAL_HELD",
            "LOT_COMMITTED",
            "LOT_AVAILABLE_QUANTITY_EXCEEDED",
            "LOT_SPLIT_NOT_ALLOWED",
            "LOT_ALREADY_ASSIGNED",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

pub(crate) fn partial_spec() -> Spec {
    let mut spec = spec(Action::CreateStockTransfer);
    spec.description = "创建整批或部分批次库存库位转储单";
    spec.input["properties"]["child_lot_id"] = id_schema();
    spec.input["properties"]["quantity"] = quantity_schema();
    spec.errors = &[
        "INVALID_ARGUMENT",
        "LOCATION_NOT_FOUND",
        "LOT_NOT_FOUND",
        "TRANSFER_ALREADY_EXISTS",
        "TRANSFER_SOURCE_CHANGED",
        "LOT_ALREADY_ASSIGNED",
        "LOT_SPLIT_NOT_ALLOWED",
        "VERSION_CONFLICT",
        "IDEMPOTENCY_CONFLICT",
    ];
    spec
}

const RECORDS: &str = "SELECT t.transfer,t.lot,s.item,s.revision,s.unit,t.source,t.destination,t.reason,t.quantity,t.posted_cursor,r.original,p.child,p.amount
    FROM stock_transfers t JOIN stock_lot_origins s ON s.run=t.run AND s.lot=t.lot
    LEFT JOIN stock_transfer_reversals r ON r.run=t.run AND r.reversal=t.transfer
    LEFT JOIN stock_lot_splits p ON p.run=t.run AND p.transfer=COALESCE(r.original,t.transfer)";

fn record(row: &Row<'_>) -> rusqlite::Result<Value> {
    let cursor: Option<i64> = row.get(9)?;
    let mut result = json!({"transfer_id":row.get::<_,String>(0)?,"lot_id":row.get::<_,String>(1)?,
        "item_id":row.get::<_,String>(2)?,"revision":row.get::<_,i64>(3)?,"unit":row.get::<_,String>(4)?,
        "source_location":row.get::<_,String>(5)?,"destination_location":row.get::<_,String>(6)?,
        "reason":row.get::<_,String>(7)?,"quantity":row.get::<_,Option<i64>>(8)?.map(quantity),
        "status":if cursor.is_some() {"posted"} else {"draft"},"posted_cursor":cursor});
    if let Some(original) = row.get::<_, Option<String>>(10)? {
        result["reverses_transfer_id"] = json!(original);
    }
    if let Some(child) = row.get::<_, Option<String>>(11)? {
        result["child_lot_id"] = json!(child);
        result["requested_quantity"] = json!(quantity(row.get(12)?));
    }
    Ok(result)
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
        "{RECORDS} WHERE t.run=?1 AND t.transfer>?2 AND t.posted_cursor IS NOT NULL
        AND (?3 IS NULL OR s.item=?3) AND (?4 IS NULL OR s.revision=?4)
        AND (?5 IS NULL OR t.source=?5 OR t.destination=?5) ORDER BY t.transfer LIMIT ?6"
    ))?;
    let mut rows = query
        .query_map(
            params![run, after, item, revision, location, limit + 1],
            record,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let next = if rows.len() > limit as usize {
        rows.pop();
        rows.last().map(|row| row["transfer_id"].clone())
    } else {
        None
    };
    Ok(json!({"ok":true,"run_id":run,"movements":rows,"next_after_movement_id":next}))
}

pub(crate) fn invoke(
    tx: Transaction<'_>,
    run: &str,
    action: Action,
    args: &Map<String, Value>,
    cursor: i64,
    order_quantity: i64,
) -> anyhow::Result<Result<Value, Rejection>> {
    for field in [
        "transfer_id",
        "movement_id",
        "reversal_id",
        "lot_id",
        "source_location",
        "destination_location",
        "child_lot_id",
    ] {
        if args
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|id| !identifier(id))
        {
            return Ok(Err(Rejection::invalid(field)));
        }
    }
    let id_field = if action == Action::ReverseInventoryMovement {
        "reversal_id"
    } else {
        "transfer_id"
    };
    let id = args[id_field].as_str().unwrap();
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
    if action == Action::ReverseInventoryMovement
        && matches!(args["movement_type"].as_str(), Some("issue" | "return"))
    {
        return match crate::operation_reversals::reverse(
            &tx,
            run,
            id,
            args,
            cursor,
            order_quantity,
        )? {
            Ok(row) => crate::reservations::finish_write(
                tx,
                run,
                key,
                &request,
                json!({"ok":true,"run_id":run,"cursor":cursor+1,"operation_reversal":row}),
            ),
            Err(error) => Ok(Err(error)),
        };
    }
    if action == Action::ReverseInventoryMovement && args["movement_type"] == "receipt" {
        return match crate::receipt_reversals::reverse(&tx, run, id, args, cursor)? {
            Ok(row) => crate::reservations::finish_write(
                tx,
                run,
                key,
                &request,
                json!({"ok":true,"run_id":run,"cursor":cursor+1,"receipt_reversal":row}),
            ),
            Err(error) => Ok(Err(error)),
        };
    }
    if action == Action::ReverseInventoryMovement {
        if let Err(error) = reverse(&tx, run, id, args, cursor)? {
            return Ok(Err(error));
        }
    } else if let Err(error) = create_or_post(&tx, run, action, id, args, cursor)? {
        return Ok(Err(error));
    }
    let transfer = tx.query_row(
        &format!("{RECORDS} WHERE t.run=?1 AND t.transfer=?2"),
        params![run, id],
        record,
    )?;
    let response = json!({"ok":true,"run_id":run,"cursor":cursor+1,"transfer":transfer});
    crate::reservations::finish_write(tx, run, key, &request, response)
}

fn create_or_post(
    tx: &Transaction<'_>,
    run: &str,
    action: Action,
    id: &str,
    args: &Map<String, Value>,
    cursor: i64,
) -> rusqlite::Result<Result<(), Rejection>> {
    let saved = tx
        .query_row(
            "SELECT lot,source,posted_cursor FROM stock_transfers WHERE run=?1 AND transfer=?2",
            params![run, id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .optional()?;
    if action == Action::CreateStockTransfer {
        if saved.is_some() {
            return Ok(Err(reject("TRANSFER_ALREADY_EXISTS", "transfer_id")));
        }
        let source = args["source_location"].as_str().unwrap();
        let destination = args["destination_location"].as_str().unwrap();
        for (field, location) in [
            ("source_location", source),
            ("destination_location", destination),
        ] {
            if !LOCATIONS.contains(&location) {
                return Ok(Err(reject("LOCATION_NOT_FOUND", field)));
            }
        }
        if source == destination {
            return Ok(Err(Rejection::invalid("destination_location")));
        }
        let lot = args["lot_id"].as_str().unwrap();
        let current = tx.query_row(
            "SELECT location,unit,(parent_lot IS NOT NULL OR EXISTS(
                SELECT 1 FROM stock_work_order_output o WHERE o.run=stock_available_lots.run AND o.receipt=stock_available_lots.receipt))
                FROM stock_available_lots WHERE run=?1 AND lot=?2 AND posted_cursor IS NOT NULL",
            params![run,lot],|row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,bool>(2)?))).optional()?;
        let Some((location, unit, split_blocked)) = current else {
            return Ok(Err(reject("LOT_NOT_FOUND", "lot_id")));
        };
        if location != source {
            return Ok(Err(reject("TRANSFER_SOURCE_CHANGED", "source_location")));
        }
        match (args.get("quantity"), args.get("child_lot_id")) {
            (Some(value), Some(child)) => {
                if split_blocked {
                    return Ok(Err(reject("LOT_SPLIT_NOT_ALLOWED", "lot_id")));
                }
                let child = child.as_str().unwrap();
                let Some(amount) = value
                    .as_str()
                    .and_then(parse_quantity)
                    .filter(|amount| unit != "each" || amount % QUANTITY_SCALE == 0)
                else {
                    return Ok(Err(Rejection::invalid("quantity")));
                };
                let occupied: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM stock_receipts WHERE run=?1 AND lot=?2)
                     OR EXISTS(SELECT 1 FROM stock_lot_splits WHERE run=?1 AND child=?2)",
                    params![run, child],
                    |row| row.get(0),
                )?;
                if occupied {
                    return Ok(Err(reject("LOT_ALREADY_ASSIGNED", "child_lot_id")));
                }
                tx.execute(
                    "INSERT INTO stock_lot_splits (run,transfer,child,amount) VALUES (?1,?2,?3,?4)",
                    params![run, id, child, amount],
                )?;
            }
            (None, None) => {}
            _ => return Ok(Err(Rejection::invalid("child_lot_id"))),
        }
        tx.execute("INSERT INTO stock_transfers (run,transfer,lot,source,destination,reason) VALUES (?1,?2,?3,?4,?5,?6)",
            params![run,id,lot,source,destination,args["reason"].as_str().unwrap()])?;
    } else {
        let Some((lot, source, posted)) = saved else {
            return Ok(Err(reject("TRANSFER_NOT_FOUND", "transfer_id")));
        };
        if posted.is_some() {
            return Ok(Err(reject("TRANSFER_ALREADY_POSTED", "transfer_id")));
        }
        let requested: Option<i64> = tx
            .query_row(
                "SELECT amount FROM stock_lot_splits WHERE run=?1 AND transfer=?2",
                params![run, id],
                |row| row.get(0),
            )
            .optional()?;
        let amount = match transferable_quantity(tx, run, &lot, &source, requested)? {
            Ok(amount) => amount,
            Err(error) => return Ok(Err(error)),
        };
        tx.execute(
            "UPDATE stock_transfers SET posted_cursor=?3,quantity=?4 WHERE run=?1 AND transfer=?2",
            params![run, id, cursor + 1, amount],
        )?;
    }
    Ok(Ok(()))
}

fn reverse(
    tx: &Transaction<'_>,
    run: &str,
    id: &str,
    args: &Map<String, Value>,
    cursor: i64,
) -> rusqlite::Result<Result<(), Rejection>> {
    let original = args["movement_id"].as_str().unwrap();
    let transfer = tx.query_row(
        "SELECT lot,source,destination,posted_cursor FROM stock_transfers WHERE run=?1 AND transfer=?2",
        params![run,original], |row| Ok((row.get::<_,String>(0)?, row.get::<_,String>(1)?,
            row.get::<_,String>(2)?,row.get::<_,Option<i64>>(3)?)),
    ).optional()?;
    let Some((lot, source, destination, posted)) = transfer else {
        return Ok(Err(reject("TRANSFER_NOT_FOUND", "movement_id")));
    };
    let Some(posted) = posted else {
        return Ok(Err(reject("TRANSFER_NOT_POSTED", "movement_id")));
    };
    let link = tx.query_row(
        "SELECT original,reversal FROM stock_transfer_reversals WHERE run=?1 AND (original=?2 OR reversal=?2)",
        params![run,original], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)),
    ).optional()?;
    if let Some((_, reversal)) = link {
        let code = if reversal == original {
            "TRANSFER_IS_REVERSAL"
        } else {
            "TRANSFER_ALREADY_REVERSED"
        };
        return Ok(Err(reject(code, "movement_id")));
    }
    let duplicate: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM stock_transfers WHERE run=?1 AND transfer=?2)",
        params![run, id],
        |row| row.get(0),
    )?;
    if duplicate {
        return Ok(Err(reject("TRANSFER_ALREADY_EXISTS", "reversal_id")));
    }
    let child: Option<String> = tx
        .query_row(
            "SELECT child FROM stock_lot_splits WHERE run=?1 AND transfer=?2",
            params![run, original],
            |row| row.get(0),
        )
        .optional()?;
    let dependent: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM stock_transfers WHERE run=?1 AND lot IN (?2,?4) AND posted_cursor>?3)
         OR EXISTS(SELECT 1 FROM stock_operation_issues i JOIN stock_reservations r
             ON r.run=i.run AND r.reservation=i.reservation WHERE i.run=?1 AND r.lot IN (?2,?4) AND i.issued_cursor>?3)",
        params![run,lot,posted,child], |row| row.get(0),
    )?;
    if dependent {
        return Ok(Err(reject("TRANSFER_HAS_DEPENDENCIES", "movement_id")));
    }
    let amount = match transferable_quantity(
        tx,
        run,
        child.as_deref().unwrap_or(&lot),
        &destination,
        None,
    )? {
        Ok(amount) => amount,
        Err(error) => return Ok(Err(error)),
    };
    tx.execute(
        "INSERT INTO stock_transfers (run,transfer,lot,source,destination,reason,posted_cursor,quantity)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![run,id,lot,destination,source,args["reason"].as_str().unwrap(),cursor+1,amount],
    )?;
    tx.execute(
        "INSERT INTO stock_transfer_reversals (run,original,reversal) VALUES (?1,?2,?3)",
        params![run, original, id],
    )?;
    Ok(Ok(()))
}

fn transferable_quantity(
    tx: &Transaction<'_>,
    run: &str,
    lot: &str,
    source: &str,
    requested: Option<i64>,
) -> rusqlite::Result<Result<i64, Rejection>> {
    let lot_stock = tx.query_row(
        "SELECT s.location,s.quantity,s.requires_inspection AND q.receipt IS NULL,
            EXISTS(SELECT 1 FROM stock_effective_lot_holds h WHERE h.run=s.run AND h.batch=s.lot),
            EXISTS(SELECT 1 FROM stock_reservations r LEFT JOIN stock_effective_return_totals t ON t.run=r.run AND t.reservation=r.reservation
                WHERE r.run=s.run AND r.lot=s.lot AND (r.status IN ('reserved','picked')
                    OR (r.status='issued' AND r.quantity>COALESCE(t.quantity,0))))
        FROM stock_available_lots s LEFT JOIN stock_receipt_releases q ON q.run=s.run AND q.receipt=s.receipt
        WHERE s.run=?1 AND s.lot=?2",
        params![run,lot],|row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,
            row.get::<_,bool>(2)?,row.get::<_,bool>(3)?,row.get::<_,bool>(4)?))).optional()?;
    let Some((location, amount, pending, held, committed)) = lot_stock else {
        return Ok(Err(reject("LOT_NOT_FOUND", "lot_id")));
    };
    if location != source {
        return Ok(Err(reject("TRANSFER_SOURCE_CHANGED", "source_location")));
    }
    for (blocked, code) in [
        (pending, "LOT_PENDING_INSPECTION"),
        (held, "MATERIAL_HELD"),
        (committed && requested.is_none(), "LOT_COMMITTED"),
    ] {
        if blocked {
            return Ok(Err(reject(code, "lot_id")));
        }
    }
    if let Some(requested) = requested {
        if requested >= amount {
            return Ok(Err(reject("LOT_SPLIT_NOT_ALLOWED", "quantity")));
        }
        if requested > amount - crate::reservations::lot_committed_quantity(tx, run, lot)? {
            return Ok(Err(reject("LOT_AVAILABLE_QUANTITY_EXCEEDED", "quantity")));
        }
        return Ok(Ok(requested));
    }
    Ok(Ok(amount))
}
