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
             "status":{"enum":["available","held","scrapped","reversed"]},
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
    let mut held =
        tx.prepare("SELECT EXISTS(SELECT 1 FROM stock_lot_holds WHERE run=?1 AND batch=?2)")?;
    let mut lots = Vec::new();
    for report in matching.iter().take(limit) {
        let blocking = if report.reversed {
            None
        } else if quality.is_some() {
            quality.as_deref()
        } else if order_held {
            Some("WORK_ORDER_HELD")
        } else {
            let mut material_held = false;
            for input in &report.material_inputs {
                material_held |=
                    held.query_row(params![run, input.lot_id], |row| row.get::<_, bool>(0))?;
            }
            material_held.then_some("MATERIAL_HELD")
        };
        let status = match (report.reversed, blocking) {
            (true, _) => "reversed",
            (false, Some("QUALITY_SCRAPPED")) => "scrapped",
            (false, Some(_)) => "held",
            (false, None) => "available",
        };
        let mut lot = json!({"wip_id":report.id,"report_id":report.id,"work_order":order,
            "source_operation":MATERIAL_INPUT_OPERATION,"quantity":report.quantity,"unit":"each","status":status,
            "serial_numbers":report.serial_numbers,"material_inputs":report.material_inputs});
        if let Some(blocking) = blocking {
            lot["blocking"] = json!(blocking);
        }
        lots.push(lot);
    }
    Ok(json!({"ok":true,"work_order":order,"lots":lots,"next_after_wip_id":next}))
}
