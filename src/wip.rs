use rusqlite::{Transaction, params};
use serde_json::{Map, Value, json};

use crate::{
    catalog::{Action, Spec, object, text},
    inventory::{id_schema, quantity_schema},
    mes::{MATERIAL_INPUT_OPERATION, Report},
    scenario::identifier,
};

pub(crate) fn spec() -> Spec {
    let input_row = object(
        &["reservation_id", "lot_id", "quantity", "unit"],
        json!({"reservation_id":id_schema(),"lot_id":id_schema(),"quantity":quantity_schema(),"unit":text()}),
    );
    let row = object(
        &[
            "wip_id",
            "report_id",
            "work_order",
            "source_operation",
            "quantity",
            "unit",
            "status",
            "serial_numbers",
            "material_inputs",
        ],
        json!({"wip_id":id_schema(),"report_id":id_schema(),"work_order":id_schema(),"source_operation":id_schema(),
             "quantity":{"type":"integer","minimum":1},"unit":{"const":"each"},
             "status":{"enum":["available","moved","consumed","held","scrapped","reversed"]},
             "movement":movement_schema(),"movement_reversals":{"type":"array","items":reversal_schema()},"consumed_by":id_schema(),
             "serial_numbers":{"type":"array","items":id_schema()},
             "material_inputs":{"type":"array","items":input_row},
             "blocking":{"enum":["WORK_ORDER_HELD","QUALITY_HOLD","QUALITY_SCRAPPED","MATERIAL_HELD"]}}),
    );
    Spec {
        name: Action::GetWip.name(),
        description: "查询工序报工产生的在制批次、实投和序列号",
        domain: "production",
        effect: "read",
        input: object(
            &["work_order"],
            json!({"work_order":id_schema(),"wip_id":id_schema(),"after_wip_id":id_schema(),
                 "limit":{"type":"integer","minimum":1,"maximum":100}}),
        ),
        output: json!({"oneOf":[object(&["ok","work_order","lots","next_after_wip_id"],
             json!({"ok":{"const":true},"work_order":id_schema(),"lots":{"type":"array","items":row},
             "next_after_wip_id":{"type":["string","null"]}})),
             object(&["ok","code"],json!({"ok":{"const":false},"code":text()}))]}),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: "none",
        errors: &["INVALID_ARGUMENT", "WORK_ORDER_NOT_FOUND", "WIP_NOT_FOUND"],
        visibility: "run",
    }
}

pub(crate) fn read(
    tx: &Transaction<'_>,
    run: &str,
    order: &str,
    order_held: bool,
    reports: &[Report],
    args: &Map<String, Value>,
) -> rusqlite::Result<Value> {
    let id = args.get("wip_id").and_then(Value::as_str);
    let after = args.get("after_wip_id").and_then(Value::as_str);
    if id.is_some_and(|id| !identifier(id))
        || after.is_some_and(|after| !identifier(after))
        || (id.is_some() && after.is_some())
    {
        return Ok(json!({"ok":false,"code":"INVALID_ARGUMENT"}));
    }
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
    let mut matching: Vec<_> = reports
        .iter()
        .filter(|report| {
            report.order == order
                && report.step == 0
                && id.is_none_or(|id| report.id == id)
                && after.is_none_or(|after| report.id.as_str() > after)
        })
        .collect();
    if id.is_some() && matching.is_empty() {
        return Ok(json!({"ok":false,"code":"WIP_NOT_FOUND"}));
    }
    matching.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    let next = (matching.len() > limit).then(|| matching[limit - 1].id.as_str());
    let quality = crate::quality::blocking(tx, run, order)?;
    let mut lots = Vec::new();
    for report in matching.iter().take(limit) {
        let blocking = if report.reversed {
            None
        } else if quality.is_some() {
            quality.as_deref()
        } else if order_held {
            Some("WORK_ORDER_HELD")
        } else {
            material_held(tx, run, report)?.then_some("MATERIAL_HELD")
        };
        let consumed = consumer(reports, order, &report.id);
        let status = match (report.reversed, blocking) {
            (true, _) => "reversed",
            (false, Some("QUALITY_SCRAPPED")) => "scrapped",
            (false, Some(_)) => "held",
            (false, None) if consumed.is_some() => "consumed",
            (false, None) if report.wip_move.is_some() => "moved",
            (false, None) => "available",
        };
        let mut lot = json!({"wip_id":report.id,"report_id":report.id,"work_order":order,
            "source_operation":MATERIAL_INPUT_OPERATION,"quantity":report.quantity,"unit":"each","status":status,
            "serial_numbers":report.serial_numbers,"material_inputs":report.material_inputs});
        if let Some(blocking) = blocking {
            lot["blocking"] = json!(blocking);
        }
        if let Some(movement) = &report.wip_move {
            lot["movement"] = json!(movement);
        }
        if !report.wip_movement_reversals.is_empty() {
            lot["movement_reversals"] = json!(report.wip_movement_reversals);
        }
        if let Some(consumed) = consumed {
            lot["consumed_by"] = json!(consumed);
        }
        lots.push(lot);
    }
    Ok(json!({"ok":true,"work_order":order,"lots":lots,"next_after_wip_id":next}))
}

pub(crate) const DESTINATION: &str = "assembly";

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Movement {
    destination_operation: String,
    cursor: i64,
    reason: String,
}

pub(crate) fn movement_schema() -> Value {
    object(
        &["destination_operation", "cursor", "reason"],
        json!({"destination_operation":id_schema(),"cursor":{"type":"integer","minimum":1},"reason":text()}),
    )
}

pub(crate) fn inputs_schema() -> Value {
    json!({"type":"array","items":id_schema(),"minItems":1,"maxItems":10,"uniqueItems":true})
}

pub(crate) fn move_spec() -> Spec {
    Spec {
        name: Action::MoveWip.name(),
        description: "按冻结工艺路线移交整批在制品",
        domain: "production",
        effect: "write",
        input: object(
            &[
                "work_order",
                "wip_id",
                "destination_operation",
                "reason",
                "expected_cursor",
                "idempotency_key",
            ],
            json!({"work_order":id_schema(),"wip_id":id_schema(),"destination_operation":id_schema(),
                "reason":text(),"expected_cursor":{"type":"integer","minimum":0,"maximum":i64::MAX},"idempotency_key":text()}),
        ),
        output: json!({"oneOf":[object(&["ok","work_order","wip_id","movement","cursor"],
            json!({"ok":{"const":true},"work_order":id_schema(),"wip_id":id_schema(),"movement":movement_schema(),
                "cursor":{"type":"integer","minimum":1}})),
            {"type":"object","required":["ok","code"],"properties":{"ok":{"const":false},"code":text()}}]}),
        preconditions: &[
            "run_open",
            "tool_enabled_in_scenario",
            "source_operation_completed",
            "quality_released",
        ],
        side_effect: "move_work_in_process",
        errors: &[
            "INVALID_ARGUMENT",
            "WORK_ORDER_NOT_FOUND",
            "WORK_ORDER_HELD",
            "WIP_NOT_FOUND",
            "WIP_NOT_AVAILABLE",
            "WIP_DESTINATION_NOT_ELIGIBLE",
            "MATERIAL_HELD",
            "QUALITY_HOLD",
            "QUALITY_SCRAPPED",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct MovementReversal {
    original_movement: Movement,
    reversed_cursor: i64,
    reason: String,
}

pub(crate) fn reversal_schema() -> Value {
    object(
        &["original_movement", "reversed_cursor", "reason"],
        json!({
        "original_movement":movement_schema(),"reversed_cursor":{"type":"integer","minimum":1},"reason":text()}),
    )
}

pub(crate) fn reverse_spec() -> Spec {
    Spec {
        name: Action::ReverseWipMovement.name(),
        description: "撤回没有有效下游耗用的在制移交",
        domain: "production",
        effect: "write",
        input: object(
            &[
                "work_order",
                "wip_id",
                "movement_cursor",
                "reason",
                "expected_cursor",
                "idempotency_key",
            ],
            json!({"work_order":id_schema(),"wip_id":id_schema(),
                "movement_cursor":{"type":"integer","minimum":1,"maximum":i64::MAX},
                "reason":text(),"expected_cursor":{"type":"integer","minimum":0,"maximum":i64::MAX},
                "idempotency_key":text()}),
        ),
        output: json!({"oneOf":[object(&["ok","work_order","wip_id","reversal","cursor"],
            json!({"ok":{"const":true},"work_order":id_schema(),"wip_id":id_schema(),
                "reversal":reversal_schema(),"cursor":{"type":"integer","minimum":1}})),
            {"type":"object","required":["ok","code"],"properties":{"ok":{"const":false},"code":text(),"cursor":{"type":"integer"}}}]}),
        preconditions: &[
            "run_open",
            "tool_enabled_in_scenario",
            "no_active_downstream_consumption",
        ],
        side_effect: "reverse_work_in_process_movement",
        errors: &[
            "INVALID_ARGUMENT",
            "WORK_ORDER_NOT_FOUND",
            "WIP_NOT_FOUND",
            "WIP_MOVEMENT_NOT_ACTIVE",
            "WIP_ALREADY_CONSUMED",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

pub(crate) fn reverse_lot(
    order: &str,
    reports: &mut [Report],
    args: &Map<String, Value>,
    cursor: i64,
) -> Value {
    let id = args["wip_id"].as_str().unwrap();
    let reason = args["reason"].as_str().unwrap();
    if !identifier(order) || !identifier(id) || reason.trim().is_empty() {
        return json!({"ok":false,"code":"INVALID_ARGUMENT"});
    }
    let Some(index) = reports
        .iter()
        .position(|report| report.order == order && report.step == 0 && report.id == id)
    else {
        return json!({"ok":false,"code":"WIP_NOT_FOUND"});
    };
    if reports[index]
        .wip_move
        .as_ref()
        .is_none_or(|movement| args["movement_cursor"].as_i64() != Some(movement.cursor))
    {
        return json!({"ok":false,"code":"WIP_MOVEMENT_NOT_ACTIVE"});
    }
    if consumer(reports, order, id).is_some() {
        return json!({"ok":false,"code":"WIP_ALREADY_CONSUMED"});
    }
    let report = &mut reports[index];
    let reversal = MovementReversal {
        original_movement: report.wip_move.take().expect("active movement"),
        reversed_cursor: cursor,
        reason: reason.into(),
    };
    let response = json!({"ok":true,"work_order":order,"wip_id":id,"reversal":reversal});
    report.wip_movement_reversals.push(reversal);
    response
}

fn material_held(tx: &Transaction<'_>, run: &str, report: &Report) -> rusqlite::Result<bool> {
    for input in &report.material_inputs {
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM stock_lot_holds WHERE run=?1 AND batch=?2)",
            params![run, input.lot_id],
            |row| row.get::<_, bool>(0),
        )? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn move_lot(
    tx: &Transaction<'_>,
    run: &str,
    order: &str,
    order_held: bool,
    reports: &mut [Report],
    args: &Map<String, Value>,
    cursor: i64,
) -> anyhow::Result<Value> {
    let id = args["wip_id"].as_str().unwrap();
    let target = args["destination_operation"].as_str().unwrap();
    if !identifier(id) || !identifier(target) {
        return Ok(json!({"ok":false,"code":"INVALID_ARGUMENT"}));
    }
    if order_held {
        return Ok(json!({"ok":false,"code":"WORK_ORDER_HELD"}));
    }
    if let Some(code) = crate::quality::blocking(tx, run, order)? {
        return Ok(json!({"ok":false,"code":code}));
    }
    let snapshot = crate::snapshots::load(tx, run, order)?;
    let eligible = target == DESTINATION
        && snapshot.is_some_and(|snapshot| {
            snapshot["route"]["operations"][target]["predecessors"]
                .as_array()
                .is_some_and(|predecessors| {
                    predecessors.iter().any(|id| id == MATERIAL_INPUT_OPERATION)
                })
        });
    if !eligible {
        return Ok(json!({"ok":false,"code":"WIP_DESTINATION_NOT_ELIGIBLE"}));
    }
    let Some(report) = reports
        .iter_mut()
        .find(|report| report.order == order && report.step == 0 && report.id == id)
    else {
        return Ok(json!({"ok":false,"code":"WIP_NOT_FOUND"}));
    };
    if report.reversed || report.wip_move.is_some() {
        return Ok(json!({"ok":false,"code":"WIP_NOT_AVAILABLE"}));
    }
    if material_held(tx, run, report)? {
        return Ok(json!({"ok":false,"code":"MATERIAL_HELD"}));
    }
    report.wip_move = Some(Movement {
        destination_operation: target.to_owned(),
        cursor,
        reason: args["reason"].as_str().unwrap().to_owned(),
    });
    Ok(json!({"ok":true,"work_order":order,"wip_id":id,"movement":report.wip_move}))
}

fn consumer<'a>(reports: &'a [Report], order: &str, id: &str) -> Option<&'a str> {
    reports
        .iter()
        .find(|report| {
            report.order == order
                && !report.reversed
                && report.wip_inputs.iter().any(|input| input == id)
        })
        .map(|report| report.id.as_str())
}

pub(crate) fn has_available(
    tx: &Transaction<'_>,
    run: &str,
    reports: &[Report],
    order: &str,
) -> rusqlite::Result<bool> {
    for report in reports.iter().filter(|report| {
        report.order == order && report.step == 0 && !report.reversed && report.wip_move.is_some()
    }) {
        if consumer(reports, order, &report.id).is_none() && !material_held(tx, run, report)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn select_inputs(
    tx: &Transaction<'_>,
    run: &str,
    reports: &[Report],
    order: &str,
    args: &Map<String, Value>,
    quantity: usize,
) -> rusqlite::Result<Result<(Vec<String>, Vec<String>), &'static str>> {
    let Some(inputs) = args.get("wip_inputs").and_then(Value::as_array) else {
        return Ok(Err("WIP_INPUTS_REQUIRED"));
    };
    let mut ids = Vec::new();
    let mut serials = Vec::new();
    for input in inputs {
        let id = input.as_str().unwrap();
        if !identifier(id) || ids.iter().any(|prior| prior == id) {
            return Ok(Err("INVALID_ARGUMENT"));
        }
        let Some(report) = reports.iter().find(|report| {
            report.order == order
                && report.step == 0
                && report.id == id
                && !report.reversed
                && report.wip_move.is_some()
        }) else {
            return Ok(Err("WIP_NOT_AT_OPERATION"));
        };
        if consumer(reports, order, id).is_some() {
            return Ok(Err("WIP_ALREADY_CONSUMED"));
        }
        if material_held(tx, run, report)? {
            return Ok(Err("MATERIAL_HELD"));
        }
        ids.push(id.to_owned());
        serials.extend(report.serial_numbers.iter().cloned());
    }
    if serials.len() != quantity {
        return Ok(Err("WIP_QUANTITY_MISMATCH"));
    }
    Ok(Ok((ids, serials)))
}
