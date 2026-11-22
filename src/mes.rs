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
        Action::SetDispatchPriority,
        "set_dispatch_priority",
        "更新工单派工优先级",
        "production",
    ),
    (
        Action::ReassignOperation,
        "reassign_operation",
        "将尚未开工的工序改派到可用工作中心",
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
        Action::PauseOperation,
        "pause_operation",
        "暂停正在执行的工序",
        "production",
    ),
    (
        Action::ResumeOperation,
        "resume_operation",
        "复查执行条件并恢复工序",
        "production",
    ),
    (
        Action::StartOperation,
        "start_operation",
        "开工",
        "production",
    ),
    (
        Action::PostOperationReport,
        "post_operation_report",
        "分笔登记当前工序生产实绩",
        "production",
    ),
    (
        Action::ListOperationReports,
        "list_operation_reports",
        "查询工序实绩与冲销情况",
        "production",
    ),
    (
        Action::ReverseOperationReport,
        "reverse_operation_report",
        "冲销无下游依赖的工序实绩",
        "production",
    ),
    (
        Action::CompleteOperation,
        "complete_operation",
        "确认实绩达标并完成工序",
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
        Action::SetDispatchPriority => &[
            "work_order",
            "priority",
            "expected_cursor",
            "idempotency_key",
        ],
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
        Action::PauseOperation => &[
            "work_order",
            "operation",
            "reason",
            "expected_cursor",
            "idempotency_key",
        ],
        Action::ResumeOperation => &[
            "work_order",
            "operation",
            "expected_cursor",
            "idempotency_key",
        ],
        Action::ReassignOperation => &[
            "work_order",
            "operation",
            "work_center",
            "expected_cursor",
            "idempotency_key",
        ],
        Action::DispatchOperation | Action::StartOperation | Action::ReportOperation => {
            &["work_order", "operation", "idempotency_key"]
        }
        Action::InspectUnits => &["work_order", "result", "idempotency_key"],
        Action::Disposition => &["work_order", "result", "reason", "idempotency_key"],
        Action::ListOperationReports => &["work_order", "operation"],
        Action::PostOperationReport => &[
            "work_order",
            "operation",
            "quantity",
            "idempotency_key",
            "expected_cursor",
        ],
        Action::ReverseOperationReport => &[
            "work_order",
            "report_id",
            "reason",
            "idempotency_key",
            "expected_cursor",
        ],
        Action::CompleteOperation => &[
            "work_order",
            "operation",
            "idempotency_key",
            "expected_cursor",
        ],
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
        Action::MesStatus
            | Action::Trace
            | Action::GetReleaseReadiness
            | Action::ListOperationReports
    );
    if let Some(equipment) = fields.get_mut("equipment_state") {
        equipment["enum"] = json!(["available", "down"]);
    }
    if action == Action::SetDispatchPriority {
        fields.insert(
            "priority".into(),
            json!({"type":"integer","minimum":0,"maximum":9}),
        );
    }
    if matches!(
        action,
        Action::DispatchOperation | Action::ReassignOperation
    ) {
        fields.insert("operation".into(), text());
        fields.insert("work_center".into(), text());
    }
    if action == Action::ReviseRoute {
        fields.insert(
            "work_centers".into(),
            json!({"type":"array","minItems":3,"maxItems":3,"items":text()}),
        );
    }
    if matches!(
        action,
        Action::StartOperation | Action::PauseOperation | Action::ResumeOperation
    ) {
        fields.insert("operation".into(), text());
    }
    if matches!(
        action,
        Action::ReportOperation | Action::PostOperationReport
    ) {
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
    if matches!(
        action,
        Action::PostOperationReport
            | Action::ListOperationReports
            | Action::ReverseOperationReport
            | Action::CompleteOperation
    ) {
        let report = object(
            &["id", "order", "step", "quantity", "batch", "reversed"],
            json!({
                "id":text(),"order":text(),"step":{"type":"integer","minimum":0},
                "quantity":{"type":"integer","minimum":1},"batch":{"type":["string","null"]},
                "reversed":{"type":"boolean"},"serial_numbers":{"type":"array","items":text()}
            }),
        );
        let success = match action {
            Action::ListOperationReports => object(
                &["ok", "work_order", "operation", "reports"],
                json!({
                    "ok":{"const":true},"work_order":text(),"operation":text(),
                    "reports":{"type":"array","items":report}
                }),
            ),
            Action::PostOperationReport => object(
                &[
                    "ok",
                    "report",
                    "serial_numbers",
                    "reported_quantity",
                    "remaining",
                    "cursor",
                ],
                json!({
                    "ok":{"const":true},"report":report,"serial_numbers":{"type":"array","items":text()},
                    "reported_quantity":{"type":"integer","minimum":0},"remaining":{"type":"integer","minimum":0},
                    "cursor":{"type":"integer","minimum":0}
                }),
            ),
            Action::CompleteOperation => object(
                &["ok", "work_order", "operation", "status", "cursor"],
                json!({
                    "ok":{"const":true},"work_order":text(),"operation":text(),"status":{"const":"completed"},
                    "cursor":{"type":"integer","minimum":0}
                }),
            ),
            _ => object(
                &["ok", "work_order", "report_id", "reversed", "cursor"],
                json!({
                    "ok":{"const":true},"work_order":text(),"report_id":text(),"reversed":{"const":true},
                    "cursor":{"type":"integer","minimum":0}
                }),
            ),
        };
        return Spec {
            name,
            description,
            domain,
            effect: if read { "read" } else { "write" },
            input: object(required, Value::Object(fields)),
            output: json!({"oneOf":[success,{"type":"object","required":["ok","code"],"properties":{"ok":{"const":false},"code":text()}}]}),
            preconditions: &["run_open", "tool_enabled_in_scenario"],
            side_effect: if read {
                "none"
            } else {
                "update_operation_actuals_or_completion"
            },
            errors: &[
                "INVALID_ARGUMENT",
                "WORK_ORDER_NOT_FOUND",
                "INVALID_STATE",
                "WORK_ORDER_HELD",
                "OPERATION_OUT_OF_SEQUENCE",
                "MATERIAL_SHORTAGE",
                "MATERIAL_HELD",
                "REPORT_EXCEEDS_REMAINING",
                "REPORT_NOT_FOUND",
                "REPORT_HAS_DEPENDENCIES",
                "RESOURCE_BUSY",
                "OPERATION_QUANTITY_INCOMPLETE",
                "VERSION_CONFLICT",
                "IDEMPOTENCY_CONFLICT",
            ],
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
            "WORK_CENTER_MISMATCH",
            "RESOURCE_BUSY",
            "CELL_NOT_CAPABLE",
            "OPERATION_NOT_DISPATCHABLE",
            "OPERATION_NOT_FOUND",
            "EQUIPMENT_UNAVAILABLE",
            "VERSION_CONFLICT",
            "ROUTE_VERSION_EXISTS",
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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strict_dispatch: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub alternate_centers: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_centers: Option<[String; 3]>,
}

#[derive(Default, Deserialize, Serialize)]
struct Order {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    assigned_centers: BTreeMap<usize, String>,
    #[serde(default, skip_serializing_if = "priority_is_default")]
    priority: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cancel_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion_confirmed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hold_reason: Option<String>,
    route_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    work_centers: Option<[String; 3]>,
    step: usize,
    phase: Phase,
    batch: Option<String>,
    units: Vec<String>,
    inspection: Option<String>,
}

fn priority_is_default(value: &u64) -> bool {
    *value == 0
}

#[derive(Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    #[default]
    Planned,
    Ready,
    Dispatched,
    Running,
    Paused,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    work_centers: Option<[String; 3]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    prior_route_versions: Vec<String>,
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
            work_centers: setup.work_centers.clone(),
            prior_route_versions: Vec::new(),
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
        if let Some(centers) = &order.work_centers {
            result["snapshot"] = json!({"route_version":order.route_version,"operations":ROUTE,"work_centers":centers});
        }
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

fn occupant<'a>(state: &'a State, center: &str, except: &str) -> Option<&'a str> {
    state
        .orders
        .iter()
        .find(|(id, order)| {
            id.as_str() != except
                && matches!(
                    order.phase,
                    Phase::Dispatched | Phase::Running | Phase::Paused
                )
                && order
                    .assigned_centers
                    .get(&order.step)
                    .is_some_and(|assigned| assigned == center)
        })
        .map(|(id, _)| id.as_str())
}

fn eligible_center(setup: &Setup, primary: &str, candidate: &str) -> bool {
    primary == candidate
        || setup
            .alternate_centers
            .get(primary)
            .is_some_and(|alternates| alternates.iter().any(|center| center == candidate))
}

pub fn operations(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    orders: &[String],
    filter: Option<&str>,
) -> anyhow::Result<Vec<crate::operations::Operation>> {
    let state = load(tx, run, setup, orders)?;
    let quality_hold: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM scenario_holds WHERE run=?1 AND batch=?2)",
        params![run, LOT],
        |row| row.get(0),
    )?;
    let mut rows = Vec::new();
    for (id, order) in &state.orders {
        if filter.is_some_and(|filter| filter != id) {
            continue;
        }
        for (index, operation) in ROUTE.iter().enumerate() {
            let (status, blocking) =
                if index < order.step || matches!(order.phase, Phase::Complete | Phase::Closed) {
                    if index == 2 && order.inspection.as_deref() == Some("fail") {
                        ("blocked", Some("QUALITY_HOLD"))
                    } else {
                        ("completed", None)
                    }
                } else if order.phase == Phase::Cancelled {
                    ("cancelled", None)
                } else if index > order.step {
                    ("pending", Some("PREDECESSOR_INCOMPLETE"))
                } else if order.hold_reason.is_some() {
                    ("held", Some("WORK_ORDER_HELD"))
                } else if order.phase == Phase::Planned {
                    ("pending", Some("ORDER_NOT_RELEASED"))
                } else {
                    let phase = match order.phase {
                        Phase::Ready => "ready",
                        Phase::Dispatched => "dispatched",
                        Phase::Running => "running",
                        Phase::Paused => "paused",
                        _ => unreachable!("operation phase handled above"),
                    };
                    let blocking = if quality_hold {
                        Some("QUALITY_HOLD")
                    } else if state.equipment_down && index < 2 {
                        Some("EQUIPMENT_UNAVAILABLE")
                    } else if index == 0
                        && state.stock < QUANTITY - reported_quantity(&state.reports, id, 0)
                    {
                        Some("MATERIAL_SHORTAGE")
                    } else {
                        None
                    };
                    (phase, blocking)
                };
            let center = order.assigned_centers.get(&index).cloned().or_else(|| {
                order
                    .work_centers
                    .as_ref()
                    .map(|centers| centers[index].clone())
            });
            let blocking = blocking.or_else(|| {
                (status == "ready"
                    && center
                        .as_deref()
                        .is_some_and(|center| occupant(&state, center, id).is_some()))
                .then_some("RESOURCE_BUSY")
            });
            rows.push(crate::operations::Operation {
                work_order: id.clone(),
                operation,
                sequence: index + 1,
                status,
                blocking,
                work_center: center,
                priority: order.priority,
                due: 0,
            });
        }
    }
    Ok(rows)
}

pub fn resource_capability(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    orders: &[String],
    center: &str,
) -> anyhow::Result<Option<Value>> {
    let state = load(tx, run, setup, orders)?;
    let mut operations = Vec::new();
    for (index, operation) in ROUTE.iter().enumerate() {
        if state
            .work_centers
            .as_ref()
            .is_some_and(|centers| eligible_center(setup, &centers[index], center))
            || state.orders.values().any(|order| {
                order
                    .work_centers
                    .as_ref()
                    .is_some_and(|centers| eligible_center(setup, &centers[index], center))
            })
        {
            operations.push(*operation);
        }
    }
    if operations.is_empty() {
        return Ok(None);
    }
    let occupied_by = occupant(&state, center, "");
    let available = !state.equipment_down
        || operations
            .iter()
            .all(|operation| *operation == "inspection");
    Ok(Some(
        json!({"work_center":center,"operations":operations,"configurations":[],
        "available":available && occupied_by.is_none(),"occupied_by":occupied_by}),
    ))
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
    if action == Action::ListOperationReports {
        let order = args["work_order"].as_str().unwrap();
        if !state.orders.contains_key(order) {
            return Ok((error("WORK_ORDER_NOT_FOUND"), false));
        }
        let operation = args["operation"].as_str().unwrap();
        let Some(step) = ROUTE.iter().position(|candidate| *candidate == operation) else {
            return Ok((error("OPERATION_OUT_OF_SEQUENCE"), false));
        };
        let reports: Vec<_> = state
            .reports
            .iter()
            .filter(|report| report.order == order && report.step == step)
            .collect();
        return Ok((
            json!({"ok":true,"work_order":order,"operation":operation,"reports":reports}),
            false,
        ));
    }
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
                Phase::Paused => vec!["resume_operation"],
                Phase::Running
                    if !held && reported_quantity(&state.reports, id, order.step) == QUANTITY =>
                {
                    vec!["complete_operation"]
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
        let version = args["route_version"].as_str().unwrap();
        if state.work_centers.is_some()
            && (version == state.route_version
                || state
                    .prior_route_versions
                    .iter()
                    .any(|previous| previous == version))
        {
            return Ok((error("ROUTE_VERSION_EXISTS"), false));
        }
        if let Some(centers) = args.get("work_centers") {
            if state.work_centers.is_none() {
                return Ok((error("INVALID_ARGUMENT"), false));
            }
            let centers: [String; 3] = serde_json::from_value(centers.clone())?;
            if !centers.iter().all(|id| crate::scenario::identifier(id)) {
                return Ok((error("INVALID_ARGUMENT"), false));
            }
            state.work_centers = Some(centers);
        }
        if state.work_centers.is_some() {
            state.prior_route_versions.push(state.route_version.clone());
        }
        state.route_version = version.into();
        json!({"ok":true,"route_version":state.route_version})
    } else if action == Action::SetEquipment {
        state.equipment_down = args["equipment_state"] == "down";
        state
            .ledger
            .push(json!({"kind":"equipment_state","equipment_state":args["equipment_state"]}));
        json!({"ok":true,"equipment_down":state.equipment_down})
    } else {
        apply(&mut state, held, setup, action, args, cursor + 1)
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
    setup: &Setup,
    action: Action,
    args: &Map<String, Value>,
    revision: i64,
) -> Value {
    let id = args["work_order"].as_str().unwrap();
    let busy = args
        .get("work_center")
        .and_then(Value::as_str)
        .and_then(|center| occupant(state, center, id))
        .map(str::to_owned);
    let reverse_busy = matches!(
        action,
        Action::ReverseReport | Action::ReverseOperationReport
    ) && state.orders.get(id).is_some_and(|order| {
        order.phase == Phase::Ready
            && order
                .step
                .checked_sub(1)
                .and_then(|step| order.assigned_centers.get(&step))
                .is_some_and(|center| occupant(state, center, id).is_some())
    });
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
        Action::DispatchOperation
            | Action::ReassignOperation
            | Action::StartOperation
            | Action::PauseOperation
            | Action::ResumeOperation
            | Action::ReportOperation
            | Action::PostOperationReport
            | Action::CompleteOperation
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
            if state.equipment_down
                && matches!(
                    order.phase,
                    Phase::Dispatched | Phase::Running | Phase::Paused
                )
            {
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
            order.work_centers = state.work_centers.clone();
            order.phase = Phase::Ready;
            if action == Action::ReleaseWorkOrder {
                let mut snapshot = json!({"route_version":order.route_version});
                if let Some(centers) = &order.work_centers {
                    snapshot["work_centers"] = json!(centers);
                    snapshot["operations"] = json!(ROUTE);
                }
                return json!({"ok":true,"work_order":id,"status":"released","snapshot":snapshot});
            }
        }
        Action::SetDispatchPriority
            if matches!(
                order.phase,
                Phase::Planned | Phase::Ready | Phase::Dispatched
            ) =>
        {
            order.priority = args["priority"].as_u64().unwrap();
            return json!({"ok":true,"work_order":id,"priority":order.priority});
        }
        Action::DispatchOperation | Action::ReassignOperation
            if (action == Action::DispatchOperation && order.phase == Phase::Ready)
                || (action == Action::ReassignOperation
                    && order.phase == Phase::Dispatched
                    && !setup.alternate_centers.is_empty()) =>
        {
            if let Some(centers) = &order.work_centers {
                let primary = &centers[order.step];
                let center = args
                    .get("work_center")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !eligible_center(setup, primary, center) {
                    return json!({"ok":false,"code":"WORK_CENTER_MISMATCH","required_work_center":primary});
                }
                if setup.strict_dispatch {
                    if held {
                        return error("QUALITY_HOLD");
                    }
                    if state.equipment_down && order.step < 2 {
                        return error("EQUIPMENT_UNAVAILABLE");
                    }
                    if order.step == 0
                        && state.stock < QUANTITY - reported_quantity(&state.reports, id, 0)
                    {
                        return error("MATERIAL_SHORTAGE");
                    }
                }
                if !setup.alternate_centers.is_empty() {
                    if let Some(owner) = busy {
                        return json!({"ok":false,"code":"RESOURCE_BUSY","occupied_by":owner});
                    }
                    if action == Action::ReassignOperation
                        && order
                            .assigned_centers
                            .get(&order.step)
                            .is_some_and(|assigned| assigned == center)
                    {
                        return error("INVALID_STATE");
                    }
                    order.assigned_centers.insert(order.step, center.into());
                }
                let kind = if action == Action::ReassignOperation {
                    "operation_reassign"
                } else {
                    "operation_dispatch"
                };
                state.ledger.push(json!({"kind":kind,"work_order":id,
                    "operation":ROUTE[order.step],"work_center":center,"route_version":order.route_version,"cursor":revision}));
            } else if args.contains_key("work_center") {
                return error("INVALID_ARGUMENT");
            }
            order.phase = Phase::Dispatched;
        }
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
        Action::PauseOperation if order.phase == Phase::Running => {
            order.phase = Phase::Paused;
            state.ledger.push(
                json!({"kind":"operation_pause","work_order":id,"operation":ROUTE[order.step],
                "reason":args["reason"],"cursor":revision}),
            );
        }
        Action::ResumeOperation if order.phase == Phase::Paused => {
            if held {
                return error("QUALITY_HOLD");
            }
            if state.equipment_down {
                return error("EQUIPMENT_UNAVAILABLE");
            }
            if order.step == 0 && state.stock < QUANTITY - reported_quantity(&state.reports, id, 0)
            {
                return error("MATERIAL_SHORTAGE");
            }
            order.phase = Phase::Running;
            state.ledger.push(json!({"kind":"operation_resume","work_order":id,"operation":ROUTE[order.step],"cursor":revision}));
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
        Action::ReportOperation | Action::PostOperationReport if order.phase == Phase::Running => {
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
            if outstanding == 0 && action == Action::ReportOperation {
                order.step += 1;
                order.phase = Phase::Ready;
            }
            let response = json!({"ok":true,"report":report,"serial_numbers":order.units,
                "reported_quantity":completed+quantity,"remaining":outstanding});
            state.reports.push(report);
            return response;
        }
        Action::CompleteOperation if order.phase == Phase::Running => {
            if held {
                return error("MATERIAL_HELD");
            }
            let remaining = QUANTITY - reported_quantity(&state.reports, id, order.step);
            if remaining > 0 {
                return json!({"ok":false,"code":"OPERATION_QUANTITY_INCOMPLETE","remaining":remaining});
            }
            let operation = ROUTE[order.step];
            state.ledger.push(json!({"kind":"operation_complete","work_order":id,"operation":operation,"cursor":revision}));
            order.step += 1;
            order.phase = Phase::Ready;
            return json!({"ok":true,"work_order":id,"operation":operation,"status":"completed"});
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
        Action::ReverseReport | Action::ReverseOperationReport => {
            if action == Action::ReverseReport
                && !matches!(order.phase, Phase::Ready | Phase::Running)
            {
                return error("INVALID_STATE");
            }
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
            if reverse_busy {
                return error("RESOURCE_BUSY");
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
            if action == Action::ReverseOperationReport {
                return json!({"ok":true,"work_order":id,"report_id":report.id,"reversed":true});
            }
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
