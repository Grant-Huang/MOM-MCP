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
             "status":{"enum":["available","moved","consumed","held","scrapped","reversed","split","merged"]},
             "parent_wip_id":id_schema(),"partitions":{"type":"array","items":partition_schema()},
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
    let mut matching: Vec<_> = lots(reports, order)
        .filter(|lot| {
            id.is_none_or(|id| lot.id() == id) && after.is_none_or(|after| lot.id() > after)
        })
        .collect();
    if id.is_some() && matching.is_empty() {
        return Ok(json!({"ok":false,"code":"WIP_NOT_FOUND"}));
    }
    matching.sort_unstable_by(|left, right| left.id().cmp(right.id()));
    let next = (matching.len() > limit).then(|| matching[limit - 1].id());
    let quality = crate::quality::blocking(tx, run, order)?;
    let mut results = Vec::new();
    for lot in matching.iter().take(limit) {
        let report = lot.report;
        let inactive = lot.inactive();
        let blocking = if inactive.is_some() {
            None
        } else if quality.is_some() {
            quality.as_deref()
        } else if order_held {
            Some("WORK_ORDER_HELD")
        } else {
            material_held(tx, run, report)?.then_some("MATERIAL_HELD")
        };
        let consumed = consumer(reports, order, lot.id());
        let status = inactive.unwrap_or_else(|| match blocking {
            Some("QUALITY_SCRAPPED") => "scrapped",
            Some(_) => "held",
            None if consumed.is_some() => "consumed",
            None if lot.movement().is_some() => "moved",
            None => "available",
        });
        let mut row = json!({"wip_id":lot.id(),"report_id":report.id,"work_order":order,
            "source_operation":MATERIAL_INPUT_OPERATION,"quantity":lot.quantity(),"unit":"each","status":status,
            "serial_numbers":lot.serials(),"material_inputs": if lot.child.is_some() { json!([]) } else { json!(report.material_inputs) }});
        if lot.child.is_some() {
            row["parent_wip_id"] = json!(report.id);
        } else if !report.wip_partitions.is_empty() {
            row["partitions"] = json!(report.wip_partitions);
        }
        if let Some(blocking) = blocking {
            row["blocking"] = json!(blocking);
        }
        if let Some(movement) = lot.movement() {
            row["movement"] = json!(movement);
        }
        let reversals = lot
            .child
            .map_or(&report.wip_movement_reversals, |(_, child)| {
                &child.reversals
            });
        if !reversals.is_empty() {
            row["movement_reversals"] = json!(reversals);
        }
        if let Some(consumed) = consumed {
            row["consumed_by"] = json!(consumed);
        }
        results.push(row);
    }
    Ok(json!({"ok":true,"work_order":order,"lots":results,"next_after_wip_id":next}))
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
    let Some(lot) = lots(reports, order).find(|lot| lot.id() == id) else {
        return json!({"ok":false,"code":"WIP_NOT_FOUND"});
    };
    if lot.inactive().is_some()
        || lot
            .movement()
            .is_none_or(|movement| args["movement_cursor"].as_i64() != Some(movement.cursor))
    {
        return json!({"ok":false,"code":"WIP_MOVEMENT_NOT_ACTIVE"});
    }
    if consumer(reports, order, id).is_some() {
        return json!({"ok":false,"code":"WIP_ALREADY_CONSUMED"});
    }
    let (movement, history) = movement_state(reports, order, id).expect("active lot");
    let reversal = MovementReversal {
        original_movement: movement.take().expect("active movement"),
        reversed_cursor: cursor,
        reason: reason.into(),
    };
    let response = json!({"ok":true,"work_order":order,"wip_id":id,"reversal":reversal});
    history.push(reversal);
    response
}

pub(crate) fn material_held(
    tx: &Transaction<'_>,
    run: &str,
    report: &Report,
) -> rusqlite::Result<bool> {
    for input in &report.material_inputs {
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM stock_effective_lot_holds WHERE run=?1 AND batch=?2)",
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
    let Some(lot) = lots(reports, order).find(|lot| lot.id() == id) else {
        return Ok(json!({"ok":false,"code":"WIP_NOT_FOUND"}));
    };
    if lot.inactive().is_some() || lot.movement().is_some() {
        return Ok(json!({"ok":false,"code":"WIP_NOT_AVAILABLE"}));
    }
    if material_held(tx, run, lot.report)? {
        return Ok(json!({"ok":false,"code":"MATERIAL_HELD"}));
    }
    let (movement, _) = movement_state(reports, order, id).expect("active lot");
    *movement = Some(Movement {
        destination_operation: target.to_owned(),
        cursor,
        reason: args["reason"].as_str().unwrap().to_owned(),
    });
    Ok(json!({"ok":true,"work_order":order,"wip_id":id,"movement":movement}))
}

pub(crate) fn consumer<'a>(reports: &'a [Report], order: &str, id: &str) -> Option<&'a str> {
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
    for lot in
        lots(reports, order).filter(|lot| lot.inactive().is_none() && lot.movement().is_some())
    {
        if consumer(reports, order, lot.id()).is_none() && !material_held(tx, run, lot.report)? {
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
        let Some(lot) = lots(reports, order)
            .find(|lot| lot.id() == id && lot.inactive().is_none() && lot.movement().is_some())
        else {
            return Ok(Err("WIP_NOT_AT_OPERATION"));
        };
        if consumer(reports, order, id).is_some() {
            return Ok(Err("WIP_ALREADY_CONSUMED"));
        }
        if material_held(tx, run, lot.report)? {
            return Ok(Err("MATERIAL_HELD"));
        }
        ids.push(id.to_owned());
        serials.extend(lot.serials().iter().cloned());
    }
    if serials.len() != quantity {
        return Ok(Err("WIP_QUANTITY_MISMATCH"));
    }
    Ok(Ok((ids, serials)))
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Child {
    pub(crate) id: String,
    pub(crate) serial_numbers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    movement: Option<Movement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    reversals: Vec<MovementReversal>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Partition {
    pub(crate) split_cursor: i64,
    split_reason: String,
    pub(crate) children: Vec<Child>,
    pub(crate) merged_cursor: Option<i64>,
    merge_reason: Option<String>,
}

pub(crate) fn partition_schema() -> Value {
    object(
        &[
            "split_cursor",
            "split_reason",
            "children",
            "merged_cursor",
            "merge_reason",
        ],
        json!({
            "split_cursor":{"type":"integer","minimum":1},"split_reason":text(),
            "merged_cursor":{"type":["integer","null"]},"merge_reason":{"type":["string","null"]},
            "children":{"type":"array","items":object(&["id","serial_numbers"],json!({
                "id":id_schema(),"serial_numbers":{"type":"array","items":id_schema(),"minItems":1},"movement":movement_schema(),
                "reversals":{"type":"array","items":reversal_schema()}
            }))}
        }),
    )
}

pub(crate) fn partition_spec(action: Action) -> Spec {
    let mut input = object(
        &[
            "work_order",
            "wip_id",
            "reason",
            "expected_cursor",
            "idempotency_key",
        ],
        json!({"work_order":id_schema(),"wip_id":id_schema(),"reason":text(),
            "expected_cursor":{"type":"integer","minimum":0,"maximum":i64::MAX},"idempotency_key":text()}),
    );
    let (field, schema, description, effect) = if action == Action::SplitWip {
        (
            "selected_serial_numbers",
            json!({"type":"array","items":id_schema(),"minItems":1,"uniqueItems":true}),
            "将工序产出按指定序列号拆成两批在制品",
            "split_work_in_process",
        )
    } else {
        (
            "split_cursor",
            json!({"type":"integer","minimum":1,"maximum":i64::MAX}),
            "将一次拆分产生的同源批次合回原批次",
            "merge_work_in_process",
        )
    };
    input["required"].as_array_mut().unwrap().push(json!(field));
    input["properties"][field] = schema;
    Spec {
        name: action.name(),
        description,
        domain: "production",
        effect: "write",
        input,
        output: json!({"oneOf":[object(&["ok","work_order","wip_id","partition","cursor"],json!({
            "ok":{"const":true},"work_order":id_schema(),"wip_id":id_schema(),
            "partition":partition_schema(),"cursor":{"type":"integer","minimum":1}})),
            {"type":"object","required":["ok","code"],"properties":{"ok":{"const":false},"code":text(),"cursor":{"type":"integer"}}}]}),
        preconditions: &[
            "run_open",
            "tool_enabled_in_scenario",
            "quality_released",
            "wip_available_at_source",
        ],
        side_effect: effect,
        errors: &[
            "INVALID_ARGUMENT",
            "WORK_ORDER_NOT_FOUND",
            "WORK_ORDER_HELD",
            "QUALITY_HOLD",
            "QUALITY_SCRAPPED",
            "MATERIAL_HELD",
            "WIP_NOT_FOUND",
            "WIP_NOT_AVAILABLE",
            "WIP_SPLIT_NOT_ACTIVE",
            "WIP_SERIAL_MISMATCH",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

pub(crate) fn active_partition(report: &Report) -> Option<&Partition> {
    report
        .wip_partitions
        .last()
        .filter(|split| split.merged_cursor.is_none())
}

pub(crate) fn partition_lot(
    tx: &Transaction<'_>,
    run: &str,
    reports: &mut [Report],
    args: &Map<String, Value>,
    cursor: i64,
    action: Action,
    order_held: bool,
) -> rusqlite::Result<Value> {
    let order = args["work_order"].as_str().unwrap();
    let id = args["wip_id"].as_str().unwrap();
    let reason = args["reason"].as_str().unwrap();
    let fail = |code| Ok(json!({"ok":false,"code":code}));
    if !identifier(id) || reason.trim().is_empty() {
        return fail("INVALID_ARGUMENT");
    }
    if order_held {
        return fail("WORK_ORDER_HELD");
    }
    if let Some(code) = crate::quality::blocking(tx, run, order)? {
        return Ok(json!({"ok":false,"code":code}));
    }
    let Some(index) = reports
        .iter()
        .position(|report| report.order == order && report.step == 0 && report.id == id)
    else {
        return fail("WIP_NOT_FOUND");
    };
    let report = &reports[index];
    if report.reversed || report.wip_move.is_some() || consumer(reports, order, id).is_some() {
        return fail("WIP_NOT_AVAILABLE");
    }
    if material_held(tx, run, report)? {
        return fail("MATERIAL_HELD");
    }
    if action == Action::SplitWip {
        if active_partition(report).is_some() {
            return fail("WIP_NOT_AVAILABLE");
        }
        let requested = args["selected_serial_numbers"].as_array().unwrap();
        let selection: std::collections::HashSet<_> =
            requested.iter().filter_map(Value::as_str).collect();
        let (selected, remaining): (Vec<_>, Vec<_>) = report
            .serial_numbers
            .iter()
            .cloned()
            .partition(|serial| selection.contains(serial.as_str()));
        if selected.len() != requested.len() || selected.is_empty() || remaining.is_empty() {
            return fail("WIP_SERIAL_MISMATCH");
        }
        let children = [selected, remaining]
            .into_iter()
            .enumerate()
            .map(|(index, serial_numbers)| Child {
                id: format!("WIP-{cursor}-{}", index + 1),
                serial_numbers,
                movement: None,
                reversals: Vec::new(),
            })
            .collect();
        reports[index].wip_partitions.push(Partition {
            split_cursor: cursor,
            split_reason: reason.into(),
            children,
            merged_cursor: None,
            merge_reason: None,
        });
    } else {
        let Some(split) = active_partition(report)
            .filter(|split| args["split_cursor"].as_i64() == Some(split.split_cursor))
        else {
            return fail("WIP_SPLIT_NOT_ACTIVE");
        };
        if split
            .children
            .iter()
            .any(|child| child.movement.is_some() || consumer(reports, order, &child.id).is_some())
        {
            return fail("WIP_NOT_AVAILABLE");
        }
        let split = reports[index].wip_partitions.last_mut().unwrap();
        split.merged_cursor = Some(cursor);
        split.merge_reason = Some(reason.into());
    }
    Ok(
        json!({"ok":true,"work_order":order,"wip_id":id,"partition":reports[index].wip_partitions.last()}),
    )
}

pub(crate) struct Lot<'a> {
    report: &'a Report,
    child: Option<(&'a Partition, &'a Child)>,
}

impl<'a> Lot<'a> {
    pub(crate) fn id(&self) -> &'a str {
        self.child.map_or(&self.report.id, |(_, child)| &child.id)
    }

    pub(crate) fn serials(&self) -> &'a [String] {
        self.child
            .map_or(&self.report.serial_numbers, |(_, child)| {
                &child.serial_numbers
            })
    }

    pub(crate) fn quantity(&self) -> usize {
        self.child.map_or(self.report.quantity, |(_, child)| {
            child.serial_numbers.len()
        })
    }

    fn movement(&self) -> Option<&Movement> {
        self.child
            .map_or(self.report.wip_move.as_ref(), |(_, child)| {
                child.movement.as_ref()
            })
    }

    pub(crate) fn inactive(&self) -> Option<&'static str> {
        match self.child {
            Some((split, _)) if split.merged_cursor.is_some() => Some("merged"),
            Some(_) => None,
            None if self.report.reversed => Some("reversed"),
            None if active_partition(self.report).is_some() => Some("split"),
            None => None,
        }
    }
}

pub(crate) fn lots<'a>(reports: &'a [Report], order: &'a str) -> impl Iterator<Item = Lot<'a>> {
    reports
        .iter()
        .filter(move |report| report.order == order && report.step == 0)
        .flat_map(|report| {
            std::iter::once(Lot {
                report,
                child: None,
            })
            .chain(report.wip_partitions.iter().flat_map(move |split| {
                split.children.iter().map(move |child| Lot {
                    report,
                    child: Some((split, child)),
                })
            }))
        })
}

fn movement_state<'a>(
    reports: &'a mut [Report],
    order: &str,
    id: &str,
) -> Option<(&'a mut Option<Movement>, &'a mut Vec<MovementReversal>)> {
    for report in reports
        .iter_mut()
        .filter(|report| report.order == order && report.step == 0 && !report.reversed)
    {
        if active_partition(report).is_some() {
            let split = report.wip_partitions.last_mut().unwrap();
            if let Some(child) = split.children.iter_mut().find(|child| child.id == id) {
                return Some((&mut child.movement, &mut child.reversals));
            }
        } else if report.id == id {
            return Some((&mut report.wip_move, &mut report.wip_movement_reversals));
        }
    }
    None
}

pub(crate) fn reconcile_spec() -> Spec {
    let count = json!({"type":"integer","minimum":0});
    let issues = object(
        &[
            "reservation_id",
            "lot_id",
            "unit",
            "issued",
            "returned",
            "consumed",
            "unconsumed",
        ],
        json!({"reservation_id":id_schema(),"lot_id":id_schema(),"unit":text(),
            "issued":quantity_schema(),"returned":quantity_schema(),"consumed":quantity_schema(),
            "unconsumed":{"type":"string","pattern":"^-?[0-9]+(\\.[0-9]{1,3})?$"}}),
    );
    let counts = object(
        &[
            "machined",
            "at_source",
            "at_destination",
            "consumed",
            "assembly_reported",
        ],
        json!({"machined":count,"at_source":count,"at_destination":count,"consumed":count,"assembly_reported":count}),
    );
    Spec {
        name: Action::ReconcileWorkOrder.name(),
        description: "核对工单实际发退料、投入量和工序在制余额",
        domain: "production",
        effect: "read",
        input: object(&["work_order"], json!({"work_order":id_schema()})),
        output: json!({"oneOf":[object(
            &["ok","work_order","balanced","wip","issues","discrepancies"],
            json!({"ok":{"const":true},"work_order":id_schema(),"balanced":{"type":"boolean"},
                "wip":counts,"issues":{"type":"array","items":issues},
                "blocking":text(),"discrepancies":{"type":"array","items":text()}})),
            object(&["ok","code"],json!({"ok":{"const":false},"code":text()}))]}),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: "none",
        errors: &["INVALID_ARGUMENT", "WORK_ORDER_NOT_FOUND"],
        visibility: "run",
    }
}

pub(crate) fn reconcile(
    tx: &Transaction<'_>,
    run: &str,
    order: &str,
    order_held: bool,
    reports: &[Report],
) -> rusqlite::Result<Value> {
    use crate::inventory::{parse_quantity, quantity};
    use std::collections::{BTreeMap, BTreeSet};

    let mut issues = Vec::new();
    let mut discrepancies = BTreeSet::new();
    let mut used = BTreeMap::new();
    let mut machined = 0;
    let mut assembly_reported = 0;
    let mut reported_serials = Vec::new();
    for report in reports
        .iter()
        .filter(|report| report.order == order && !report.reversed)
    {
        if report.step == 0 {
            machined += report.quantity;
        } else if report.step == 1 {
            assembly_reported += report.quantity;
            reported_serials.extend(report.serial_numbers.iter().map(String::as_str));
        }
        for input in &report.material_inputs {
            *used.entry(input.reservation_id.as_str()).or_insert(0) +=
                parse_quantity(&input.quantity).expect("validated material quantity");
        }
    }
    let mut query = tx.prepare(
        "SELECT r.reservation,r.lot,s.unit,r.quantity,COALESCE(t.quantity,0)
         FROM stock_reservations r
         JOIN stock_lot_origins s ON s.run=r.run AND s.lot=r.lot
         LEFT JOIN stock_effective_return_totals t ON t.run=r.run AND t.reservation=r.reservation
         WHERE r.run=?1 AND r.work_order=?2 AND r.status='issued' ORDER BY r.reservation",
    )?;
    let mut rows = query.query(params![run, order])?;
    while let Some(row) = rows.next()? {
        let reservation: String = row.get(0)?;
        let issued: i64 = row.get(3)?;
        let returned: i64 = row.get(4)?;
        let consumed = used.remove(reservation.as_str()).unwrap_or(0);
        let remaining = issued - returned - consumed;
        if remaining < 0 {
            discrepancies.insert("ISSUE_OVERCONSUMED");
        }
        let unconsumed = if remaining < 0 {
            format!("-{}", quantity(-remaining))
        } else {
            quantity(remaining)
        };
        issues.push(
            json!({"reservation_id":reservation,"lot_id":row.get::<_,String>(1)?,
            "unit":row.get::<_,String>(2)?,"issued":quantity(issued),"returned":quantity(returned),
            "consumed":quantity(consumed),"unconsumed":unconsumed}),
        );
    }
    if !used.is_empty() {
        discrepancies.insert("REPORTED_ISSUE_MISSING");
    }
    let (mut at_source, mut at_destination, mut consumed) = (0, 0, 0);
    let mut consumed_serials = Vec::new();
    let mut material_blocked = false;
    for lot in lots(reports, order).filter(|lot| lot.inactive().is_none()) {
        if consumer(reports, order, lot.id()).is_some() {
            consumed += lot.quantity();
            consumed_serials.extend(lot.serials().iter().map(String::as_str));
        } else if lot.movement().is_some() {
            at_destination += lot.quantity();
        } else {
            at_source += lot.quantity();
        }
        material_blocked |= material_held(tx, run, lot.report)?;
    }
    if machined != at_source + at_destination + consumed {
        discrepancies.insert("WIP_QUANTITY_MISMATCH");
    }
    reported_serials.sort_unstable();
    consumed_serials.sort_unstable();
    if assembly_reported != consumed || reported_serials != consumed_serials {
        discrepancies.insert("ASSEMBLY_INPUT_MISMATCH");
    }
    let mut result = json!({"ok":true,"work_order":order,"balanced":discrepancies.is_empty(),
        "wip":{"machined":machined,"at_source":at_source,"at_destination":at_destination,
            "consumed":consumed,"assembly_reported":assembly_reported},
        "issues":issues,"discrepancies":discrepancies});
    if let Some(blocking) = crate::quality::blocking(tx, run, order)?
        .or_else(|| order_held.then(|| "WORK_ORDER_HELD".into()))
        .or_else(|| material_blocked.then(|| "MATERIAL_HELD".into()))
    {
        result["blocking"] = json!(blocking);
    }
    Ok(result)
}
