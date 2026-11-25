use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Map, Value, json};

use crate::{
    catalog::{Action, Spec, object, text},
    inventory::id_schema,
    scenario::{Rejection, identifier, reject},
};

pub(crate) fn spec(action: Action) -> Spec {
    let hold = action == Action::HoldStockLot;
    let mut properties = json!({"hold_id":id_schema(),"reason":text(),
        "expected_cursor":{"type":"integer","minimum":0,"maximum":i64::MAX},"idempotency_key":text()});
    let mut required = vec!["hold_id", "reason", "expected_cursor", "idempotency_key"];
    if hold {
        properties["lot_id"] = id_schema();
        required.push("lot_id");
    }
    Spec {
        name: action.name(),
        description: if hold {
            "冻结原料批次及使用该批次的在制投入"
        } else {
            "按冻结编号放行库存批次"
        },
        domain: "quality",
        effect: "write",
        input: object(&required, properties),
        output: json!({"oneOf":[object(&["ok","run_id","hold_id","lot_id","held","reason","cursor"],
            json!({"ok":{"const":true},"run_id":text(),"hold_id":id_schema(),"lot_id":id_schema(),
                "held":{"type":"boolean"},"reason":text(),"cursor":{"type":"integer","minimum":1}})),
            object(&["ok","code","field"],json!({"ok":{"const":false},"code":text(),"field":text()}))]}),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: action.name(),
        errors: &[
            "INVALID_ARGUMENT",
            "LOT_NOT_FOUND",
            "LOT_ALREADY_HELD",
            "HOLD_ALREADY_EXISTS",
            "HOLD_NOT_ACTIVE",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

pub(crate) fn held(tx: &Transaction<'_>, run: &str, lot: &str) -> rusqlite::Result<bool> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM stock_scoped_lot_holds WHERE run=?1 AND batch=?2)",
        params![run, lot],
        |row| row.get(0),
    )
}

pub(crate) fn invoke(
    tx: Transaction<'_>,
    run: &str,
    action: Action,
    args: &Map<String, Value>,
    cursor: i64,
) -> anyhow::Result<Result<Value, Rejection>> {
    let id = args["hold_id"].as_str().unwrap();
    let reason = args["reason"].as_str().unwrap();
    if !identifier(id) {
        return Ok(Err(Rejection::invalid("hold_id")));
    }
    if reason.trim().is_empty() {
        return Ok(Err(Rejection::invalid("reason")));
    }
    let key = args["idempotency_key"].as_str().unwrap();
    let request = json!({"action":action,"arguments":args}).to_string();
    let saved = tx
        .query_row(
            "SELECT request,response FROM scenario_requests WHERE run=?1 AND key=?2",
            params![run, key],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
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
            "SELECT lot,released_cursor IS NULL FROM stock_quality_holds WHERE run=?1 AND hold=?2",
            params![run, id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
        )
        .optional()?;
    let hold = action == Action::HoldStockLot;
    let lot = if hold {
        if saved.is_some() {
            return Ok(Err(reject("HOLD_ALREADY_EXISTS", "hold_id")));
        }
        let lot = args["lot_id"].as_str().unwrap();
        if !identifier(lot) {
            return Ok(Err(Rejection::invalid("lot_id")));
        }
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM stock_available_lots
            WHERE run=?1 AND lot=?2 AND posted_cursor IS NOT NULL)",
            params![run, lot],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(Err(reject("LOT_NOT_FOUND", "lot_id")));
        }
        if held(&tx, run, lot)? {
            return Ok(Err(reject("LOT_ALREADY_HELD", "lot_id")));
        }
        tx.execute("INSERT INTO stock_quality_holds(run,hold,lot,reason,held_cursor) VALUES(?1,?2,?3,?4,?5)",
            params![run,id,lot,reason,cursor+1])?;
        lot.to_owned()
    } else {
        let Some((lot, true)) = saved else {
            return Ok(Err(reject("HOLD_NOT_ACTIVE", "hold_id")));
        };
        tx.execute("UPDATE stock_quality_holds SET release_reason=?3,released_cursor=?4 WHERE run=?1 AND hold=?2",
            params![run,id,reason,cursor+1])?;
        lot
    };
    let effective = held(&tx, run, &lot)?;
    crate::reservations::finish_write(
        tx,
        run,
        key,
        &request,
        json!({"ok":true,"run_id":run,"hold_id":id,"lot_id":lot,"held":effective,"reason":reason,"cursor":cursor+1}),
    )
}
