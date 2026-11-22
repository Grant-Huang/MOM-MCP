use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::catalog::{Action, Spec, object, text};

const ROUTE: [&str; 3] = ["machining", "assembly", "inspection"];
const LOT: &str = "LOT-1";
const QUANTITY: usize = 10;

pub const TOOLS: &[(Action, &str, &str, &str)] = &[
    (
        Action::MesStatus,
        "mes_status",
        "查询制造状态与阻塞原因",
        "production",
    ),
    (
        Action::GetReleaseReadiness,
        "get_release_readiness",
        "预检工单下达条件与阻塞原因",
        "production",
    ),
    (
        Action::ReleaseWorkOrder,
        "release_work_order",
        "下达工单并固化执行定义",
        "production",
    ),
    (
        Action::HoldWorkOrder,
        "hold_work_order",
        "暂停工单执行并记录原因",
        "production",
    ),
    (
        Action::ResumeWorkOrder,
        "resume_work_order",
        "复查阻塞并恢复工单执行",
        "production",
    ),
    (
        Action::CancelWorkOrder,
        "cancel_work_order",
        "取消尚无执行投入的工单",
        "production",
    ),
    (
        Action::CompleteWorkOrder,
        "complete_work_order",
        "确认工单实绩和质量处置已完成",
        "production",
    ),
    (
        Action::CloseWorkOrder,
        "close_work_order",
        "结清并关闭已确认完工的工单",
        "production",
    ),
    (
        Action::ReleaseOrder,
        "release_order",
        "下达工单并冻结工艺版本",
        "production",
    ),
    (
        Action::ReviseRoute,
        "revise_route",
        "更新待下达工单的主数据版本",
        "production",
    ),
    (
        Action::SetEquipment,
        "set_equipment",
        "设置模拟设备状态",
        "production",
    ),
    (
        Action::DispatchOperation,
        "dispatch_operation",
        "派工",
        "production",
    ),
    (
        Action::IssueMaterial,
        "issue_material",
        "指定领料批次",
        "inventory",
    ),
    (
        Action::StartOperation,
        "start_operation",
        "开工",
        "production",
    ),
    (
        Action::ReportOperation,
        "report_operation",
        "报工并记录用料与实绩",
        "production",
    ),
    (
        Action::InspectUnits,
        "inspect_units",
        "终检并隔离不合格产品",
        "quality",
    ),
    (
        Action::Disposition,
        "disposition",
        "复检放行或报废",
        "quality",
    ),
    (Action::CloseOrder, "close_order", "关单", "production"),
    (
        Action::Trace,
        "trace",
        "按原料批次或成品序列号双向追溯",
        "inventory",
    ),
    (
        Action::ReverseReport,
        "reverse_report",
        "冲销报工并恢复库存",
        "production",
    ),
];

pub fn spec(action: Action) -> Spec {
    let &(_, name, description, domain) = TOOLS.iter().find(|entry| entry.0 == action).unwrap();
    let required: &[&str] = match action {
        Action::MesStatus | Action::GetReleaseReadiness => &["work_order"],
        Action::Trace => &["subject"],
        Action::ReviseRoute => &["route_version", "idempotency_key"],
        Action::SetEquipment => &["equipment_state", "idempotency_key"],
        Action::CancelWorkOrder => &["work_order", "idempotency_key", "expected_cursor", "reason"],
        Action::CompleteWorkOrder | Action::CloseWorkOrder => {
            &["work_order", "idempotency_key", "expected_cursor"]
        }
        Action::ReleaseWorkOrder | Action::ResumeWorkOrder => {
            &["work_order", "idempotency_key", "expected_cursor"]
        }
        Action::HoldWorkOrder => &["work_order", "idempotency_key", "expected_cursor", "reason"],
        Action::ReleaseOrder | Action::CloseOrder => &["work_order", "idempotency_key"],
        Action::IssueMaterial => &["work_order", "batch", "idempotency_key"],
        Action::DispatchOperation | Action::StartOperation | Action::ReportOperation => {
            &["work_order", "operation", "idempotency_key"]
        }
        Action::InspectUnits => &["work_order", "result", "idempotency_key"],
        Action::Disposition => &["work_order", "result", "reason", "idempotency_key"],
        Action::ReverseReport => &["work_order", "report_id", "reason", "idempotency_key"],
        _ => unreachable!(),
    };
    let mut fields: Map<String, Value> = required
        .iter()
        .map(|field| (field.to_string(), text()))
        .collect();
    for (field, values) in [
        ("operation", &ROUTE[..]),
        (
            "result",
            if action == Action::InspectUnits {
                &["pass", "fail"][..]
            } else {
                &["pass", "scrap"][..]
            },
        ),
    ] {
        if let Some(rule) = fields.get_mut(field) {
            rule["enum"] = json!(values);
        }
    }
    let read = matches!(
        action,
        Action::MesStatus | Action::Trace | Action::GetReleaseReadiness
    );
    if let Some(equipment) = fields.get_mut("equipment_state") {
        equipment["enum"] = json!(["available", "down"]);
    }
    if action == Action::ReportOperation {
        fields.insert(
            "quantity".into(),
            json!({"type":"integer","minimum":1,"maximum":QUANTITY}),
        );
    }
    if !read {
        fields.insert(
            "expected_cursor".into(),
            json!({"type":"integer","minimum":0,"maximum":i64::MAX}),
        );
    }
    if matches!(
        action,
        Action::CancelWorkOrder | Action::CompleteWorkOrder | Action::CloseWorkOrder
    ) {
        return Spec {
            name,
            description,
            domain,
            effect: "write",
            input: object(required, Value::Object(fields)),
            output: json!({"type":"object","required":["ok"],"properties":{
                "ok":{"type":"boolean"},"work_order":text(),"status":text(),"cancel_reason":text(),
                "cursor":{"type":"integer","minimum":0},"code":text()
            }}),
            preconditions: &["run_open", "work_order_lifecycle_conditions_met"],
            side_effect: "update_work_order_lifecycle",
            errors: &[
                "INVALID_ARGUMENT",
                "WORK_ORDER_NOT_FOUND",
                "INVALID_STATE",
                "WORK_ORDER_HELD",
                "ORDER_HAS_EXECUTION",
                "QUALITY_HOLD",
                "COMPLETION_REQUIRED",
                "DELIVERY_REQUIRED",
                "VERSION_CONFLICT",
                "IDEMPOTENCY_CONFLICT",
            ],
            visibility: "run",
        };
    }
    if matches!(action, Action::HoldWorkOrder | Action::ResumeWorkOrder) {
        return Spec {
            name,
            description,
            domain,
            effect: "write",
            input: object(required, Value::Object(fields)),
            output: json!({"type":"object","required":["ok"],"properties":{
                "ok":{"type":"boolean"},"work_order":text(),"status":text(),"hold_reason":text(),
                "cursor":{"type":"integer","minimum":0},"code":text()
            }}),
            preconditions: &["run_open", "work_order_released"],
            side_effect: "update_work_order_execution_hold",
            errors: &[
                "INVALID_ARGUMENT",
                "WORK_ORDER_NOT_FOUND",
                "INVALID_STATE",
                "WORK_ORDER_HELD",
                "QUALITY_HOLD",
                "EQUIPMENT_UNAVAILABLE",
                "MATERIAL_SHORTAGE",
                "VERSION_CONFLICT",
                "IDEMPOTENCY_CONFLICT",
            ],
            visibility: "run",
        };
    }
    if action == Action::ReleaseWorkOrder {
        return Spec {
            name,
            description,
            domain,
            effect: "write",
            input: object(required, Value::Object(fields)),
            output: json!({"type":"object","required":["ok"],"properties":{
                "ok":{"type":"boolean"},"work_order":text(),"status":{"const":"released"},
                "cursor":{"type":"integer","minimum":0},"snapshot":{"type":"object"},"code":text()
            }}),
            preconditions: &["run_open", "work_order_planned", "release_conditions_met"],
            side_effect: "release_order_and_freeze_execution_definition",
            errors: &[
                "INVALID_ARGUMENT",
                "WORK_ORDER_NOT_FOUND",
                "INVALID_STATE",
                "MATERIAL_SHORTAGE",
                "MATERIAL_HELD",
                "CELL_NOT_CAPABLE",
                "VERSION_CONFLICT",
                "IDEMPOTENCY_CONFLICT",
            ],
            visibility: "run",
        };
    }
    if action == Action::GetReleaseReadiness {
        return Spec {
            name,
            description,
            domain,
            effect: "read",
            input: object(required, Value::Object(fields)),
            output: json!({"oneOf":[
                object(&["ok","work_order","ready","blocking","cursor"], json!({
                    "ok":{"const":true},"work_order":text(),"ready":{"type":"boolean"},
                    "blocking":{"enum":[null,"INVALID_STATE","MATERIAL_SHORTAGE","MATERIAL_HELD","CELL_NOT_CAPABLE"]},
                    "cursor":{"type":"integer","minimum":0},"route_version":text(),
                    "eligible_cells":{"type":"array","items":text()},
                    "missing":{"type":"object","additionalProperties":{"type":"integer","minimum":1}}
                })),
                object(&["ok","code"],json!({"ok":{"const":false},"code":text(),"field":text()}))
            ]}),
            preconditions: &["run_open", "tool_enabled_in_scenario"],
            side_effect: "none",
            errors: &["INVALID_ARGUMENT", "WORK_ORDER_NOT_FOUND"],
            visibility: "run",
        };
    }
    Spec {
        name,
        description,
        domain,
        effect: if read { "read" } else { "write" },
        input: object(required, Value::Object(fields)),
        output: json!({"type":"object","required":["ok"],"properties":{"ok":{"type":"boolean"},"code":{"type":"string"}}}),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: if read {
            "none"
        } else {
            "update_manufacturing_state"
        },
        errors: &[
            "MATERIAL_SHORTAGE",
            "MATERIAL_HELD",
            "OPERATION_OUT_OF_SEQUENCE",
            "INVALID_STATE",
            "QUALITY_HOLD",
            "IDEMPOTENCY_CONFLICT",
            "REPORT_EXCEEDS_REMAINING",
            "REPORT_HAS_DEPENDENCIES",
        ],
        visibility: "run",
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    pub initial_stock: usize,
}

#[derive(Default, Deserialize, Serialize)]
struct Order {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cancel_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion_confirmed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hold_reason: Option<String>,
    route_version: Option<String>,
    step: usize,
    phase: Phase,
    batch: Option<String>,
    units: Vec<String>,
    inspection: Option<String>,
}

#[derive(Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    #[default]
    Planned,
    Ready,
    Dispatched,
    Running,
    Complete,
    Closed,
    Cancelled,
}

#[derive(Deserialize, Serialize)]
struct Report {
    id: String,
    order: String,
    step: usize,
    batch: Option<String>,
    quantity: usize,
    reversed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    serial_numbers: Vec<String>,
}

#[derive(Deserialize, Serialize)]
struct State {
    stock: usize,
    route_version: String,
    orders: BTreeMap<String, Order>,
    reports: Vec<Report>,
    ledger: Vec<Value>,
    receipt_cursor: Option<usize>,
    #[serde(default)]
    equipment_down: bool,
}

impl State {
    fn new(setup: &Setup, orders: &[String]) -> Self {
        Self {
            stock: setup.initial_stock,
            route_version: "route-1".into(),
            orders: orders
                .iter()
                .map(|id| (id.clone(), Order::default()))
                .collect(),
            reports: vec![],
            ledger: vec![],
            receipt_cursor: None,
            equipment_down: false,
        }
    }
}

fn load(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    orders: &[String],
) -> anyhow::Result<State> {
    let stored: Option<String> = tx
        .query_row("SELECT state FROM mes_runs WHERE run=?1", [run], |row| {
            row.get(0)
        })
        .optional()?;
    let mut state: State = match stored {
        Some(value) => serde_json::from_str(&value)?,
        None => return Ok(State::new(setup, orders)),
    };
    for id in orders {
        state.orders.entry(id.clone()).or_default();
    }
    Ok(state)
}

fn store(tx: &Transaction<'_>, run: &str, state: &State) -> anyhow::Result<()> {
    tx.execute(
        "INSERT INTO mes_runs VALUES (?1,?2) ON CONFLICT(run) DO UPDATE SET state=excluded.state",
        params![run, serde_json::to_string(state)?],
    )?;
    Ok(())
}

pub fn project_receipts(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    orders: &[String],
    events: &[mom_des::Event],
) -> anyhow::Result<()> {
    let Some(event) = events.iter().find(|event| event.kind == "receipt_arrival") else {
        return Ok(());
    };
    let mut state = load(tx, run, setup, orders)?;
    if state.receipt_cursor.is_none() {
        state.stock += QUANTITY;
        state.receipt_cursor = Some(event.cursor);
        state.ledger.push(
            json!({"kind":"receipt","batch":LOT,"delta":QUANTITY,"event_cursor":event.cursor}),
        );
        store(tx, run, &state)?;
        tx.execute(
            "UPDATE scenario_runs SET cursor=cursor+1 WHERE id=?1",
            [run],
        )?;
    }
    Ok(())
}

fn error(code: &str) -> Value {
    json!({"ok":false,"code":code})
}

pub fn order_status(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    orders: &[String],
    id: &str,
) -> anyhow::Result<Option<Value>> {
    let state = load(tx, run, setup, orders)?;
    Ok(state.orders.get(id).map(|order| {
        let mut result = json!({"ok":true,"run_id":run,"work_order":id,"status":order.phase});
        if let Some(reason) = &order.hold_reason {
            result["status"] = json!("held");
            result["hold_reason"] = json!(reason);
        }
        if let Some(reason) = &order.cancel_reason {
            result["cancel_reason"] = json!(reason);
        }
        if order.completion_confirmed == Some(true) {
            result["completion_confirmed"] = json!(true);
        }
        result
    }))
}

pub fn invoke(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    orders: &[String],
    action: Action,
    args: &Map<String, Value>,
    cursor: i64,
) -> anyhow::Result<(Value, bool)> {
    let mut state = load(tx, run, setup, orders)?;
    if action == Action::Trace {
        let subject = args["subject"].as_str().unwrap();
        let links: Vec<_> = state
            .orders
            .iter()
            .filter_map(|(id, order)| {
                let batch = order.batch.as_deref()?;
                let units: Vec<_> = order
                    .units
                    .iter()
                    .filter(|unit| subject == batch || subject == unit.as_str())
                    .collect();
                (!units.is_empty())
                    .then(|| json!({"work_order":id,"batch":batch,"serial_numbers":units}))
            })
            .collect();
        return Ok((json!({"ok":true,"links":links}), false));
    }
    let held: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM scenario_holds WHERE run=?1 AND batch=?2)",
        params![run, LOT],
        |row| row.get(0),
    )?;
    if action == Action::GetReleaseReadiness {
        let id = args["work_order"].as_str().unwrap();
        let Some(order) = state.orders.get(id) else {
            return Ok((error("WORK_ORDER_NOT_FOUND"), false));
        };
        let blocking = release_blocker(&order.phase, state.stock, held);
        return Ok((
            json!({"ok":true,"work_order":id,"ready":blocking.is_none(),
            "blocking":blocking,"route_version":state.route_version,"cursor":cursor}),
            false,
        ));
    }
    if action == Action::MesStatus {
        let id = args["work_order"].as_str().unwrap();
        let Some(order) = state.orders.get(id) else {
            return Ok((error("WORK_ORDER_NOT_FOUND"), false));
        };
        let blocking = if order.hold_reason.is_some() {
            Some("WORK_ORDER_HELD")
        } else if held || order.inspection.as_deref() == Some("fail") {
            Some("QUALITY_HOLD")
        } else if order.phase == Phase::Planned && state.stock < QUANTITY {
            Some("MATERIAL_SHORTAGE")
        } else if state.equipment_down && order.phase == Phase::Dispatched {
            Some("EQUIPMENT_UNAVAILABLE")
        } else {
            None
        };
        let available = if order.hold_reason.is_some() {
            vec!["resume_work_order"]
        } else {
            match order.phase {
                Phase::Planned if !held && state.stock >= QUANTITY => vec!["release_order"],
                Phase::Ready => {
                    let mut actions = vec!["dispatch_operation"];
                    if order.step > 0 {
                        actions.push("reverse_report");
                    }
                    actions
                }
                Phase::Dispatched if order.step == 2 && !held => vec!["inspect_units"],
                Phase::Dispatched if !held => {
                    let mut actions = Vec::new();
                    if order.step == 0 && state.stock >= QUANTITY {
                        actions.push("issue_material");
                    }
                    if !state.equipment_down && order.batch.is_some() {
                        actions.push("start_operation");
                    }
                    actions
                }
                Phase::Running if !held && (order.step != 0 || state.stock > 0) => {
                    vec!["report_operation"]
                }
                Phase::Complete if order.inspection.as_deref() == Some("fail") => {
                    vec!["disposition"]
                }
                Phase::Complete if !held || order.inspection.as_deref() == Some("scrap") => {
                    vec!["close_order"]
                }
                Phase::Complete => vec!["disposition"],
                _ => Vec::new(),
            }
        };
        let reports: Vec<_> = state
            .reports
            .iter()
            .filter(|report| report.order == id)
            .collect();
        return Ok((
            json!({"ok":true,"work_order":id,"state":order,"on_hand":state.stock,"location":"RAW-01",
            "blocking":blocking,"available_actions":available,"equipment_down":state.equipment_down,"route_master":state.route_version,"ledger":state.ledger,"reports":reports,"cursor":cursor}),
            false,
        ));
    }

    let key = args["idempotency_key"].as_str().unwrap();
    let request = json!({"action":action,"arguments":args}).to_string();
    let saved: Option<(String, String)> = tx
        .query_row(
            "SELECT request,response FROM scenario_requests WHERE run=?1 AND key=?2",
            params![run, key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((original, response)) = saved {
        return Ok((
            if original == request {
                serde_json::from_str(&response)?
            } else {
                error("IDEMPOTENCY_CONFLICT")
            },
            false,
        ));
    }
    if args
        .get("expected_cursor")
        .and_then(Value::as_i64)
        .is_some_and(|expected| expected != cursor)
    {
        return Ok((
            json!({"ok":false,"code":"VERSION_CONFLICT","cursor":cursor}),
            false,
        ));
    }
    let mut response = if action == Action::ReviseRoute {
        state.route_version = args["route_version"].as_str().unwrap().into();
        json!({"ok":true,"route_version":state.route_version})
    } else if action == Action::SetEquipment {
        state.equipment_down = args["equipment_state"] == "down";
        state
            .ledger
            .push(json!({"kind":"equipment_state","equipment_state":args["equipment_state"]}));
        json!({"ok":true,"equipment_down":state.equipment_down})
    } else {
        apply(&mut state, held, action, args, cursor + 1)
    };
    if response["ok"] != true {
        return Ok((response, false));
    }
    if action == Action::Disposition && args["result"] == "pass" {
        tx.execute(
            "DELETE FROM scenario_holds WHERE run=?1 AND batch=?2",
            params![run, LOT],
        )?;
    }
    response["cursor"] = (cursor + 1).into();
    store(tx, run, &state)?;
    tx.execute(
        "UPDATE scenario_runs SET cursor=cursor+1 WHERE id=?1",
        [run],
    )?;
    tx.execute(
        "INSERT INTO scenario_requests VALUES (?1,?2,?3,?4)",
        params![run, key, request, response.to_string()],
    )?;
    Ok((response, true))
}

fn release_blocker(phase: &Phase, stock: usize, held: bool) -> Option<&'static str> {
    if *phase != Phase::Planned {
        Some("INVALID_STATE")
    } else if stock < QUANTITY {
        Some("MATERIAL_SHORTAGE")
    } else if held {
        Some("MATERIAL_HELD")
    } else {
        None
    }
}

fn apply(
    state: &mut State,
    held: bool,
    action: Action,
    args: &Map<String, Value>,
    revision: i64,
) -> Value {
    let id = args["work_order"].as_str().unwrap();
    let Some(order) = state.orders.get_mut(id) else {
        return error("WORK_ORDER_NOT_FOUND");
    };
    if order.hold_reason.is_some()
        && !matches!(action, Action::ResumeWorkOrder | Action::CancelWorkOrder)
    {
        return error("WORK_ORDER_HELD");
    }
    if matches!(
        action,
        Action::DispatchOperation | Action::StartOperation | Action::ReportOperation
    ) && ROUTE.get(order.step).copied() != args["operation"].as_str()
    {
        return error("OPERATION_OUT_OF_SEQUENCE");
    }
    match action {
        Action::HoldWorkOrder
            if matches!(
                order.phase,
                Phase::Ready | Phase::Dispatched | Phase::Running
            ) =>
        {
            let reason = args["reason"].as_str().unwrap();
            order.hold_reason = Some(reason.into());
            return json!({"ok":true,"work_order":id,"status":"held","hold_reason":reason});
        }
        Action::ResumeWorkOrder if order.hold_reason.is_some() => {
            if held {
                return error("QUALITY_HOLD");
            }
            if state.equipment_down && matches!(order.phase, Phase::Dispatched | Phase::Running) {
                return error("EQUIPMENT_UNAVAILABLE");
            }
            if order.step == 0 && state.stock < QUANTITY - reported_quantity(&state.reports, id, 0)
            {
                return error("MATERIAL_SHORTAGE");
            }
            order.hold_reason = None;
            return json!({"ok":true,"work_order":id,"status":order.phase});
        }
        Action::CancelWorkOrder if !matches!(order.phase, Phase::Closed | Phase::Cancelled) => {
            if !matches!(
                order.phase,
                Phase::Planned | Phase::Ready | Phase::Dispatched
            ) || order.batch.is_some()
                || order.step > 0
                || state.reports.iter().any(|report| report.order == id)
            {
                return error("ORDER_HAS_EXECUTION");
            }
            let reason = args["reason"].as_str().unwrap();
            order.phase = Phase::Cancelled;
            order.hold_reason = None;
            order.cancel_reason = Some(reason.into());
            return json!({"ok":true,"work_order":id,"status":"cancelled","cancel_reason":reason});
        }
        Action::CompleteWorkOrder | Action::CloseWorkOrder if order.phase == Phase::Complete => {
            if order.inspection.as_deref() == Some("fail")
                || (held && order.inspection.as_deref() != Some("scrap"))
            {
                return error("QUALITY_HOLD");
            }
            if action == Action::CompleteWorkOrder && order.completion_confirmed.is_none() {
                order.completion_confirmed = Some(true);
                return json!({"ok":true,"work_order":id,"status":"completed"});
            }
            if action == Action::CloseWorkOrder {
                if order.completion_confirmed.is_none() {
                    return error("COMPLETION_REQUIRED");
                }
                order.phase = Phase::Closed;
                return json!({"ok":true,"work_order":id,"status":"closed"});
            }
            return error("INVALID_STATE");
        }
        Action::ReleaseOrder | Action::ReleaseWorkOrder => {
            if let Some(code) = release_blocker(&order.phase, state.stock, held) {
                return if code == "MATERIAL_SHORTAGE" {
                    shortage(state.stock)
                } else {
                    error(code)
                };
            }
            order.route_version = Some(state.route_version.clone());
            order.phase = Phase::Ready;
            if action == Action::ReleaseWorkOrder {
                return json!({"ok":true,"work_order":id,"status":"released",
                    "snapshot":{"route_version":order.route_version}});
            }
        }
        Action::DispatchOperation if order.phase == Phase::Ready => order.phase = Phase::Dispatched,
        Action::IssueMaterial if order.phase == Phase::Dispatched && order.step == 0 => {
            if args["batch"] != LOT {
                return error("MATERIAL_NOT_FOUND");
            }
            if held {
                return error("MATERIAL_HELD");
            }
            if state.stock < QUANTITY {
                return shortage(state.stock);
            }
            order.batch = Some(LOT.into());
        }
        Action::StartOperation if order.phase == Phase::Dispatched => {
            if state.equipment_down {
                return error("EQUIPMENT_UNAVAILABLE");
            }
            if order.batch.is_none() {
                return error("MATERIAL_NOT_ISSUED");
            }
            if held {
                return error("MATERIAL_HELD");
            }
            if order.step == 2 {
                return error("USE_INSPECTION");
            }
            order.phase = Phase::Running;
        }
        Action::ReportOperation if order.phase == Phase::Running => {
            if held {
                return error("MATERIAL_HELD");
            }
            let completed = reported_quantity(&state.reports, id, order.step);
            let remaining = QUANTITY - completed;
            let quantity = args
                .get("quantity")
                .and_then(Value::as_u64)
                .map_or(remaining, |value| value as usize);
            if quantity > remaining {
                return json!({"ok":false,"code":"REPORT_EXCEEDS_REMAINING","remaining":remaining});
            }
            if order.step == 0 && state.stock < quantity {
                return shortage_for(quantity, state.stock);
            }
            let serial_numbers = if order.step == 0 {
                let used: usize = state
                    .reports
                    .iter()
                    .filter(|report| report.order == id && report.step == 0)
                    .map(|report| report.quantity)
                    .sum();
                (used + 1..=used + quantity)
                    .map(|n| format!("{id}-{n:02}"))
                    .collect()
            } else {
                vec![]
            };
            let report = Report {
                id: format!("report-{revision}"),
                order: id.into(),
                step: order.step,
                quantity,
                batch: order.batch.clone(),
                reversed: false,
                serial_numbers,
            };
            if order.step == 0 {
                state.stock -= quantity;
                order.units.extend(report.serial_numbers.iter().cloned());
                state.ledger.push(json!({"kind":"consumption","report_id":report.id,"batch":LOT,"delta":-(quantity as i64)}));
            }
            let outstanding = remaining - quantity;
            if outstanding == 0 {
                order.step += 1;
                order.phase = Phase::Ready;
            }
            let response = json!({"ok":true,"report":report,"serial_numbers":order.units,
                "reported_quantity":completed+quantity,"remaining":outstanding});
            state.reports.push(report);
            return response;
        }
        Action::InspectUnits if order.phase == Phase::Dispatched && order.step == 2 => {
            if held {
                return error("MATERIAL_HELD");
            }
            order.inspection = Some(args["result"].as_str().unwrap().into());
            order.phase = Phase::Complete;
            state
                .ledger
                .push(json!({"kind":"inspection","order":id,"result":args["result"]}));
        }
        Action::Disposition
            if order.phase == Phase::Complete
                && (order.inspection.as_deref() == Some("fail") || held) =>
        {
            order.inspection = Some(args["result"].as_str().unwrap().into());
            state.ledger.push(json!({"kind":"disposition","order":id,"result":args["result"],"reason":args["reason"]}));
        }
        Action::CloseOrder if order.phase == Phase::Complete => {
            if order.inspection.as_deref() == Some("fail")
                || (held && order.inspection.as_deref() != Some("scrap"))
            {
                return error("QUALITY_HOLD");
            }
            order.phase = Phase::Closed;
        }
        Action::ReverseReport if matches!(order.phase, Phase::Ready | Phase::Running) => {
            let Some(index) = state
                .reports
                .iter()
                .position(|report| report.id == args["report_id"] && report.order == id)
            else {
                return error("REPORT_NOT_FOUND");
            };
            let report = &state.reports[index];
            let same_step = order.phase == Phase::Running && order.step == report.step;
            let previous_step = order.phase == Phase::Ready && order.step == report.step + 1;
            let later_report = state.reports[index + 1..]
                .iter()
                .any(|later| later.order == id && later.step == report.step && !later.reversed);
            if report.reversed || !(same_step || previous_step) || later_report {
                return error("REPORT_HAS_DEPENDENCIES");
            }
            let report = &mut state.reports[index];
            let delta = if report.step == 0 {
                state.stock += report.quantity;
                order.units.truncate(order.units.len() - report.quantity);
                report.quantity
            } else {
                0
            };
            state
                .ledger
                .push(json!({"kind":"reversal","report_id":report.id,
                "reason":args["reason"],"batch":report.batch,"delta":delta}));
            report.reversed = true;
            order.step = report.step;
            order.phase = Phase::Running;
        }
        _ => return error("INVALID_STATE"),
    }
    json!({"ok":true,"work_order":id,"state":order})
}

fn reported_quantity(reports: &[Report], order: &str, step: usize) -> usize {
    reports
        .iter()
        .filter(|report| report.order == order && report.step == step && !report.reversed)
        .map(|report| report.quantity)
        .sum()
}

fn shortage(on_hand: usize) -> Value {
    shortage_for(QUANTITY, on_hand)
}

fn shortage_for(required: usize, on_hand: usize) -> Value {
    json!({"ok":false,"code":"MATERIAL_SHORTAGE","batch":LOT,"missing":required-on_hand,"location":"RAW-01"})
}
