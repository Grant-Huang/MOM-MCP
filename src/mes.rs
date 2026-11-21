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
        Action::MesStatus => &["work_order"],
        Action::Trace => &["subject"],
        Action::ReviseRoute => &["route_version", "idempotency_key"],
        Action::SetEquipment => &["equipment_state", "idempotency_key"],
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
    let read = matches!(action, Action::MesStatus | Action::Trace);
    if let Some(equipment) = fields.get_mut("equipment_state") {
        equipment["enum"] = json!(["available", "down"]);
    }
    if !read {
        fields.insert(
            "expected_cursor".into(),
            json!({"type":"integer","minimum":0,"maximum":i64::MAX}),
        );
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
}

#[derive(Deserialize, Serialize)]
struct Report {
    id: String,
    order: String,
    step: usize,
    batch: Option<String>,
    quantity: usize,
    reversed: bool,
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
    Ok(stored.map_or_else(
        || Ok(State::new(setup, orders)),
        |state| serde_json::from_str(&state),
    )?)
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
    Ok(state
        .orders
        .get(id)
        .map(|order| json!({"ok":true,"run_id":run,"work_order":id,"status":order.phase})))
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
    if action == Action::MesStatus {
        let id = args["work_order"].as_str().unwrap();
        let Some(order) = state.orders.get(id) else {
            return Ok((error("WORK_ORDER_NOT_FOUND"), false));
        };
        let blocking = if held || order.inspection.as_deref() == Some("fail") {
            Some("QUALITY_HOLD")
        } else if order.phase == Phase::Planned && state.stock < QUANTITY {
            Some("MATERIAL_SHORTAGE")
        } else if state.equipment_down && order.phase == Phase::Dispatched {
            Some("EQUIPMENT_UNAVAILABLE")
        } else {
            None
        };
        let available = match order.phase {
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
            Phase::Running if !held && (order.step != 0 || state.stock >= QUANTITY) => {
                vec!["report_operation"]
            }
            Phase::Complete if order.inspection.as_deref() == Some("fail") => vec!["disposition"],
            Phase::Complete if !held || order.inspection.as_deref() == Some("scrap") => {
                vec!["close_order"]
            }
            Phase::Complete => vec!["disposition"],
            _ => Vec::new(),
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
    if matches!(
        action,
        Action::DispatchOperation | Action::StartOperation | Action::ReportOperation
    ) && ROUTE.get(order.step).copied() != args["operation"].as_str()
    {
        return error("OPERATION_OUT_OF_SEQUENCE");
    }
    match action {
        Action::ReleaseOrder if order.phase == Phase::Planned => {
            if state.stock < QUANTITY {
                return shortage(state.stock);
            }
            if held {
                return error("MATERIAL_HELD");
            }
            order.route_version = Some(state.route_version.clone());
            order.phase = Phase::Ready;
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
            if order.step == 0 && state.stock < QUANTITY {
                return shortage(state.stock);
            }
            let report = Report {
                id: format!("report-{revision}"),
                order: id.into(),
                step: order.step,
                quantity: QUANTITY,
                batch: order.batch.clone(),
                reversed: false,
            };
            if order.step == 0 {
                state.stock -= QUANTITY;
                order.units = (1..=QUANTITY).map(|n| format!("{id}-{n:02}")).collect();
                state.ledger.push(json!({"kind":"consumption","report_id":report.id,"batch":LOT,"delta":-(QUANTITY as i64)}));
            }
            order.step += 1;
            order.phase = Phase::Ready;
            let response = json!({"ok":true,"report":report,"serial_numbers":order.units});
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
        Action::ReverseReport if order.phase == Phase::Ready => {
            let Some(report) = state
                .reports
                .iter_mut()
                .find(|r| r.id == args["report_id"] && r.order == id)
            else {
                return error("REPORT_NOT_FOUND");
            };
            if report.reversed || order.step != report.step + 1 {
                return error("REPORT_HAS_DEPENDENCIES");
            }
            if report.step == 0 {
                state.stock += report.quantity;
                order.units.clear();
                state.ledger.push(json!({"kind":"reversal","report_id":report.id,"reason":args["reason"],"batch":LOT,"delta":report.quantity}));
            } else {
                state.ledger.push(json!({"kind":"reversal","report_id":report.id,"reason":args["reason"],"delta":0}));
            }
            report.reversed = true;
            order.step -= 1;
            order.phase = Phase::Running;
        }
        _ => return error("INVALID_STATE"),
    }
    json!({"ok":true,"work_order":id,"state":order})
}

fn shortage(on_hand: usize) -> Value {
    json!({"ok":false,"code":"MATERIAL_SHORTAGE","batch":LOT,"missing":QUANTITY-on_hand,"location":"RAW-01"})
}
