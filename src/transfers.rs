use rusqlite::{OptionalExtension, Row, Transaction, params};
use serde_json::{Map, Value, json};

use crate::{
    catalog::{Action, Spec, object, text},
    inventory::{LOCATIONS, id_schema, quantity, quantity_schema},
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
            "posted_cursor":{"type":["integer","null"],"minimum":1}}),
    )
}

pub(crate) fn spec(action: Action) -> Spec {
    let mut fields = json!({"transfer_id":id_schema(),"expected_cursor":{"type":"integer","minimum":0,"maximum":i64::MAX},"idempotency_key":text()});
    let mut required = vec!["transfer_id", "expected_cursor", "idempotency_key"];
    let description = if action == Action::CreateStockTransfer {
        required.extend([
            "lot_id",
            "source_location",
            "destination_location",
            "reason",
        ]);
        for field in ["lot_id", "source_location", "destination_location"] {
            fields[field] = id_schema();
        }
        fields["reason"] = text();
        "创建整批库存库位转储单"
    } else {
        "核对整批库存可用性并过账库位转储"
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
            "LOT_PENDING_INSPECTION",
            "MATERIAL_HELD",
            "LOT_COMMITTED",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

const RECORDS: &str = "SELECT t.transfer,t.lot,s.item,s.revision,s.unit,t.source,t.destination,t.reason,t.quantity,t.posted_cursor
    FROM stock_transfers t JOIN stock_receipts s ON s.run=t.run AND s.lot=t.lot";

fn record(row: &Row<'_>) -> rusqlite::Result<Value> {
    let cursor: Option<i64> = row.get(9)?;
    Ok(
        json!({"transfer_id":row.get::<_,String>(0)?,"lot_id":row.get::<_,String>(1)?,
        "item_id":row.get::<_,String>(2)?,"revision":row.get::<_,i64>(3)?,"unit":row.get::<_,String>(4)?,
        "source_location":row.get::<_,String>(5)?,"destination_location":row.get::<_,String>(6)?,
        "reason":row.get::<_,String>(7)?,"quantity":row.get::<_,Option<i64>>(8)?.map(quantity),
        "status":if cursor.is_some() {"posted"} else {"draft"},"posted_cursor":cursor}),
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
) -> anyhow::Result<Result<Value, Rejection>> {
    for field in [
        "transfer_id",
        "lot_id",
        "source_location",
        "destination_location",
    ] {
        if args
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|id| !identifier(id))
        {
            return Ok(Err(Rejection::invalid(field)));
        }
    }
    let id = args["transfer_id"].as_str().unwrap();
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
        let current: Option<String> = tx.query_row(
            "SELECT location FROM stock_current_lots WHERE run=?1 AND lot=?2 AND posted_cursor IS NOT NULL",
            params![run,lot],|row| row.get(0)).optional()?;
        match current.as_deref() {
            None => return Ok(Err(reject("LOT_NOT_FOUND", "lot_id"))),
            Some(location) if location != source => {
                return Ok(Err(reject("TRANSFER_SOURCE_CHANGED", "source_location")));
            }
            _ => {}
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
        let amount = match transferable_quantity(&tx, run, &lot, &source)? {
            Ok(amount) => amount,
            Err(error) => return Ok(Err(error)),
        };
        tx.execute(
            "UPDATE stock_transfers SET posted_cursor=?3,quantity=?4 WHERE run=?1 AND transfer=?2",
            params![run, id, cursor + 1, amount],
        )?;
    }
    let transfer = tx.query_row(
        &format!("{RECORDS} WHERE t.run=?1 AND t.transfer=?2"),
        params![run, id],
        record,
    )?;
    let response = json!({"ok":true,"run_id":run,"cursor":cursor+1,"transfer":transfer});
    crate::reservations::finish_write(tx, run, key, &request, response)
}

fn transferable_quantity(
    tx: &Transaction<'_>,
    run: &str,
    lot: &str,
    source: &str,
) -> rusqlite::Result<Result<i64, Rejection>> {
    let (location,amount,pending,held,committed) = tx.query_row(
        "SELECT s.location,s.quantity,s.requires_inspection AND q.receipt IS NULL,
            EXISTS(SELECT 1 FROM scenario_holds h WHERE h.run=s.run AND h.batch=s.lot),
            EXISTS(SELECT 1 FROM stock_reservations r LEFT JOIN stock_return_totals t ON t.run=r.run AND t.reservation=r.reservation
                WHERE r.run=s.run AND r.lot=s.lot AND (r.status IN ('reserved','picked')
                    OR (r.status='issued' AND r.quantity>COALESCE(t.quantity,0))))
        FROM stock_current_lots s LEFT JOIN stock_receipt_releases q ON q.run=s.run AND q.receipt=s.receipt
        WHERE s.run=?1 AND s.lot=?2",
        params![run,lot],|row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,
            row.get::<_,bool>(2)?,row.get::<_,bool>(3)?,row.get::<_,bool>(4)?)))?;
    if location != source {
        return Ok(Err(reject("TRANSFER_SOURCE_CHANGED", "source_location")));
    }
    for (blocked, code) in [
        (pending, "LOT_PENDING_INSPECTION"),
        (held, "MATERIAL_HELD"),
        (committed, "LOT_COMMITTED"),
    ] {
        if blocked {
            return Ok(Err(reject(code, "lot_id")));
        }
    }
    Ok(Ok(amount))
}
