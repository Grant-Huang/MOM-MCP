use std::collections::BTreeMap;

use mom_des::Event;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    catalog::{Action, Spec, object, text},
    simulation::{Command, Simulation},
};

pub const TOOLS: &[(Action, &str, &str, &str)] = &[
    (
        Action::AssemblyKit,
        "assembly_kit",
        "按单装配齐套检查",
        "inventory",
    ),
    (
        Action::AssemblySubstitute,
        "assembly_substitute",
        "校验并选择替代件",
        "inventory",
    ),
    (
        Action::AssemblyDispatch,
        "assembly_dispatch",
        "按工位能力派工",
        "production",
    ),
    (
        Action::AssemblyBuild,
        "assembly_build",
        "装配扣料与实装记录",
        "production",
    ),
    (
        Action::AssemblyPriority,
        "assembly_priority",
        "调整终测队列优先级",
        "production",
    ),
    (
        Action::AssemblyStartTest,
        "assembly_start_test",
        "占用共享终测台",
        "quality",
    ),
    (
        Action::AssemblyFinishTest,
        "assembly_finish_test",
        "收集模拟终测结果",
        "quality",
    ),
    (
        Action::AssemblyRepair,
        "assembly_repair",
        "登记装配返修",
        "quality",
    ),
    (
        Action::AssemblyDeliver,
        "assembly_deliver",
        "生成交付反馈",
        "production",
    ),
    (
        Action::AssemblyTrace,
        "assembly_trace",
        "查询测试台、实装与交付追溯",
        "production",
    ),
];

pub fn spec(action: Action) -> Spec {
    let &(_, name, description, domain) = TOOLS.iter().find(|entry| entry.0 == action).unwrap();
    let read = matches!(action, Action::AssemblyKit | Action::AssemblyTrace);
    let mut required = vec![if action == Action::AssemblyTrace {
        "subject"
    } else {
        "work_order"
    }];
    if !read {
        required.push("idempotency_key");
    }
    required.extend(match action {
        Action::AssemblySubstitute => &["part"][..],
        Action::AssemblyDispatch => &["cell"][..],
        Action::AssemblyPriority => &["priority"][..],
        Action::AssemblyRepair => &["reason"][..],
        _ => &[],
    });
    let fields = required
        .iter()
        .map(|&name| {
            (
                name.into(),
                if name == "priority" {
                    json!({"type":"integer","minimum":0,"maximum":9})
                } else {
                    text()
                },
            )
        })
        .collect::<Map<_, _>>();
    Spec {
        name,
        description,
        domain,
        effect: if read { "read" } else { "write" },
        input: object(&required, Value::Object(fields)),
        output: json!({"type":"object","required":["ok"],"properties":{"ok":{"type":"boolean"},"code":{"type":"string"}}}),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: if read {
            "none"
        } else {
            "update_assembly_state"
        },
        errors: &[
            "INVALID_STATE",
            "MATERIAL_SHORTAGE",
            "SUBSTITUTE_NOT_QUALIFIED",
            "TEST_BENCH_BUSY",
            "TEST_PRIORITY_BLOCKED",
            "TEST_NOT_READY",
            "TEST_PASS_REQUIRED",
        ],
        visibility: "run",
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exclusive_dispatch: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub issued_report_inputs: bool,
    stock: BTreeMap<String, usize>,
    orders: BTreeMap<String, OrderDefinition>,
    cells: BTreeMap<String, Vec<String>>,
    bom: BTreeMap<String, usize>,
    substitutes: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderDefinition {
    configuration: String,
    serial: String,
    due_in_minutes: u64,
}

impl Setup {
    pub fn add_order(
        &mut self,
        id: &str,
        template: &str,
        serial: &str,
    ) -> Result<(), &'static str> {
        let source = self.orders.get(template).ok_or("TEMPLATE_NOT_FOUND")?;
        if self.orders.values().any(|order| order.serial == serial) {
            return Err("SERIAL_IN_USE");
        }
        self.orders.insert(
            id.into(),
            OrderDefinition {
                configuration: source.configuration.clone(),
                due_in_minutes: source.due_in_minutes,
                serial: serial.into(),
            },
        );
        Ok(())
    }

    pub fn validate(
        &self,
        orders: &[String],
        model: &str,
        parameters: &Map<String, Value>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            model == mom_model_assembly::ID,
            "assembly requires configure_to_order"
        );
        anyhow::ensure!(
            self.orders.len() == orders.len()
                && orders.iter().all(|id| self.orders.contains_key(id)),
            "assembly.orders must match work_orders"
        );
        anyhow::ensure!(
            self.bom.contains_key("DRIVE-STD"),
            "assembly BOM requires DRIVE-STD"
        );
        anyhow::ensure!(
            self.bom.iter().all(|(part, quantity)| *quantity > 0
                && *quantity <= 1000
                && self.stock.contains_key(part)),
            "invalid assembly BOM"
        );
        anyhow::ensure!(
            self.stock.values().all(|quantity| *quantity <= 1_000_000),
            "invalid assembly stock"
        );
        anyhow::ensure!(
            self.substitutes
                .iter()
                .all(|(part, qualification)| !self.bom.contains_key(part)
                    && self.stock.contains_key(part)
                    && !qualification.trim().is_empty()),
            "invalid assembly substitute"
        );
        let serials: std::collections::BTreeSet<_> =
            self.orders.values().map(|order| &order.serial).collect();
        anyhow::ensure!(
            serials.len() == orders.len()
                && self.orders.values().all(|order| !order.serial.is_empty()
                    && order.due_in_minutes > 0
                    && self
                        .cells
                        .values()
                        .any(|configurations| configurations.contains(&order.configuration))),
            "invalid assembly serial, due time, or cell configuration"
        );
        if let Some(order) = parameters.get("fault_order").and_then(Value::as_str) {
            anyhow::ensure!(
                self.orders.contains_key(order),
                "fault_order references unknown work order"
            );
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Order {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    build_reports: Vec<crate::mes::Report>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    build_pause_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cancel_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion_confirmed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hold_reason: Option<String>,
    phase: Phase,
    substitute: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    released_parts: Option<BTreeMap<String, usize>>,
    cell: Option<String>,
    priority: u64,
    installed: BTreeMap<String, usize>,
    tests: Vec<Test>,
    repairs: Vec<String>,
    delivery: Option<Value>,
}

#[derive(Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    #[default]
    Planned,
    Released,
    Dispatched,
    Building,
    BuildPaused,
    Queued,
    Testing,
    Failed,
    Passed,
    Delivered,
    Closed,
    Cancelled,
}

#[derive(Serialize, Deserialize)]
struct Test {
    ticket: i64,
    start_minute: u64,
    end_minute: Option<u64>,
    result: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct State {
    stock: BTreeMap<String, usize>,
    orders: BTreeMap<String, Order>,
    bench: Option<String>,
    received: bool,
}

pub(crate) fn reported_material_quantity(
    tx: &Transaction<'_>,
    run: &str,
    order: &str,
    reservation: &str,
) -> anyhow::Result<i64> {
    let saved: Option<String> = tx
        .query_row(
            "SELECT state FROM assembly_runs WHERE run=?1",
            [run],
            |row| row.get(0),
        )
        .optional()?;
    let Some(saved) = saved else { return Ok(0) };
    let state: State = serde_json::from_str(&saved)?;
    Ok(state.orders.get(order).map_or(0, |order_state| {
        crate::mes::material_used(&order_state.build_reports, order, reservation)
    }))
}

pub(crate) fn genealogy(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    action: Action,
    args: &Map<String, Value>,
) -> anyhow::Result<Value> {
    let reports: Vec<_> = load(tx, run, setup)?
        .orders
        .into_values()
        .flat_map(|order| order.build_reports)
        .collect();
    crate::genealogy::read(
        tx,
        run,
        action,
        &reports,
        args,
        crate::genealogy::Detail::Standard,
    )
}

fn has_build(order: &Order) -> bool {
    !order.installed.is_empty() || order.build_reports.iter().any(|report| !report.reversed)
}

fn load(tx: &Transaction<'_>, run: &str, setup: &Setup) -> anyhow::Result<State> {
    let stored: Option<String> = tx
        .query_row(
            "SELECT state FROM assembly_runs WHERE run=?1",
            [run],
            |row| row.get(0),
        )
        .optional()?;
    let mut state: State = match stored {
        Some(value) => serde_json::from_str(&value)?,
        None => State {
            stock: setup.stock.clone(),
            orders: BTreeMap::new(),
            bench: None,
            received: false,
        },
    };
    for id in setup.orders.keys() {
        state.orders.entry(id.clone()).or_default();
    }
    Ok(state)
}

fn save(tx: &Transaction<'_>, run: &str, state: &State) -> anyhow::Result<()> {
    tx.execute("INSERT INTO assembly_runs VALUES (?1,?2) ON CONFLICT(run) DO UPDATE SET state=excluded.state",
        params![run,serde_json::to_string(state)?])?;
    Ok(())
}

pub fn project_receipt(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    events: &[Event],
) -> anyhow::Result<()> {
    if !events
        .iter()
        .any(|event| event.kind == "assembly_receipt" && event.payload["part"] == "DRIVE-STD")
    {
        return Ok(());
    }
    let mut state = load(tx, run, setup)?;
    if !state.received {
        *state.stock.get_mut("DRIVE-STD").unwrap() += 1;
        state.received = true;
        save(tx, run, &state)?;
        tx.execute(
            "UPDATE scenario_runs SET cursor=cursor+1 WHERE id=?1",
            [run],
        )?;
    }
    Ok(())
}

fn failure(code: &str) -> Value {
    json!({"ok":false,"code":code})
}

fn required(setup: &Setup, order: &Order) -> BTreeMap<String, usize> {
    if let Some(parts) = &order.released_parts {
        return parts.clone();
    }
    let mut required = setup.bom.clone();
    if let Some(part) = &order.substitute {
        let quantity = required.remove("DRIVE-STD").unwrap();
        required.insert(part.clone(), quantity);
    }
    required
}

fn missing(
    setup: &Setup,
    required: &BTreeMap<String, usize>,
    stock: &BTreeMap<String, usize>,
) -> BTreeMap<String, usize> {
    if setup.issued_report_inputs {
        return BTreeMap::new();
    }

    required
        .iter()
        .filter_map(|(part, &quantity)| {
            let short = quantity.saturating_sub(stock[part]);
            (short > 0).then(|| (part.clone(), short))
        })
        .collect()
}

fn queue<'a>(state: &'a State, setup: &Setup) -> Vec<&'a String> {
    let mut queued: Vec<_> = state
        .orders
        .iter()
        .filter(|(_, order)| order.phase == Phase::Queued && order.hold_reason.is_none())
        .map(|(id, _)| id)
        .collect();
    queued.sort_by_key(|id| {
        (
            std::cmp::Reverse(state.orders[*id].priority),
            setup.orders[*id].due_in_minutes,
            *id,
        )
    });
    queued
}

pub fn status(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    id: &str,
) -> anyhow::Result<Option<Value>> {
    let state = load(tx, run, setup)?;
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

pub fn operations(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    filter: Option<&str>,
) -> anyhow::Result<Vec<crate::operations::Operation>> {
    let state = load(tx, run, setup)?;
    let test_queue = queue(&state, setup);
    let mut rows = Vec::new();
    for (id, order) in &state.orders {
        if filter.is_some_and(|filter| filter != id) {
            continue;
        }
        let built =
            has_build(order) && !matches!(order.phase, Phase::Building | Phase::BuildPaused);
        let (build, test) = if order.phase == Phase::Cancelled {
            (("cancelled", None), ("cancelled", None))
        } else {
            let build = if built {
                ("completed", None)
            } else if order.hold_reason.is_some() {
                ("held", Some("WORK_ORDER_HELD"))
            } else if order.phase == Phase::Planned {
                ("pending", Some("ORDER_NOT_RELEASED"))
            } else {
                let shortages = missing(setup, &required(setup, order), &state.stock);
                (
                    match order.phase {
                        Phase::Dispatched => "dispatched",
                        Phase::Building => "running",
                        Phase::BuildPaused => "paused",
                        _ => "ready",
                    },
                    (!has_build(order) && !shortages.is_empty()).then_some("MATERIAL_SHORTAGE"),
                )
            };
            let test = if !built {
                ("pending", Some("PREDECESSOR_INCOMPLETE"))
            } else if order.hold_reason.is_some() {
                ("held", Some("WORK_ORDER_HELD"))
            } else {
                match order.phase {
                    Phase::Queued if state.bench.is_some() => ("ready", Some("TEST_BENCH_BUSY")),
                    Phase::Queued if test_queue.first().copied() != Some(id) => {
                        ("ready", Some("TEST_PRIORITY_BLOCKED"))
                    }
                    Phase::Queued => ("ready", None),
                    Phase::Testing => ("running", None),
                    Phase::Failed => ("blocked", Some("QUALITY_HOLD")),
                    _ => ("completed", None),
                }
            };
            (build, test)
        };
        for (index, (operation, center, (status, blocking))) in [
            ("build", order.cell.clone(), build),
            ("final_test", Some("TEST-1".into()), test),
        ]
        .into_iter()
        .enumerate()
        {
            rows.push(crate::operations::Operation {
                work_order: id.clone(),
                operation,
                sequence: index + 1,
                status,
                blocking,
                work_center: center,
                priority: order.priority,
                due: setup.orders[id].due_in_minutes,
            });
        }
    }
    Ok(rows)
}

fn cell_occupant<'a>(state: &'a State, cell: &str, except: &str) -> Option<&'a str> {
    state
        .orders
        .iter()
        .find(|(id, order)| {
            id.as_str() != except
                && matches!(
                    order.phase,
                    Phase::Dispatched | Phase::Building | Phase::BuildPaused
                )
                && order.cell.as_deref() == Some(cell)
        })
        .map(|(id, _)| id.as_str())
}

pub fn resource_capability(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    center: &str,
) -> anyhow::Result<Option<Value>> {
    if center == "TEST-1" {
        let state = load(tx, run, setup)?;
        return Ok(Some(
            json!({"work_center":center,"operations":["final_test"],"configurations":[],
            "available":state.bench.is_none(),"occupied_by":state.bench}),
        ));
    }
    let state = load(tx, run, setup)?;
    let occupied_by = setup
        .exclusive_dispatch
        .then(|| cell_occupant(&state, center, ""))
        .flatten();
    Ok(setup.cells.get(center).map(|configurations| {
        json!({"work_center":center,
        "operations":["build"],"configurations":configurations,"available":occupied_by.is_none(),"occupied_by":occupied_by})
    }))
}

pub fn release_readiness(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    id: &str,
    cursor: i64,
) -> anyhow::Result<Value> {
    let state = load(tx, run, setup)?;
    let Some(order) = state.orders.get(id) else {
        return Ok(failure("WORK_ORDER_NOT_FOUND"));
    };
    let cells: Vec<_> = setup
        .cells
        .iter()
        .filter(|(_, configurations)| configurations.contains(&setup.orders[id].configuration))
        .map(|(cell, _)| cell)
        .collect();
    let shortages = missing(setup, &required(setup, order), &state.stock);
    let result = dispatch_blocker(order.phase == Phase::Planned, !cells.is_empty(), &shortages);
    Ok(json!({"ok":true,"work_order":id,"ready":result.is_none(),
        "blocking":result,"cursor":cursor,"eligible_cells":cells,"missing":shortages}))
}

fn dispatch_blocker(
    valid_phase: bool,
    capable: bool,
    shortages: &BTreeMap<String, usize>,
) -> Option<&'static str> {
    if !valid_phase {
        Some("INVALID_STATE")
    } else if !capable {
        Some("CELL_NOT_CAPABLE")
    } else if !shortages.is_empty() {
        Some("MATERIAL_SHORTAGE")
    } else {
        None
    }
}

pub fn invoke(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    action: Action,
    args: &Map<String, Value>,
    cursor: i64,
    require_release: bool,
    simulation: &mut Simulation,
    rules: &[mom_des::Rule],
    model: &dyn mom_des::Model,
) -> anyhow::Result<(Value, bool)> {
    let mut state = load(tx, run, setup)?;
    if action == Action::AssemblyTrace {
        let subject = args["subject"].as_str().unwrap();
        let links: Vec<_> = state.orders.iter().filter_map(|(id,order)| {
            let spec = &setup.orders[id];
            let inputs: Vec<_> = order.build_reports.iter().filter(|report| !report.reversed)
                .flat_map(|report| &report.material_inputs).collect();
            if subject != id && subject != spec.serial && !order.installed.contains_key(subject)
                && !inputs.iter().any(|input| input.lot_id == subject) {
                return None;
            }
            let mut link = json!({"work_order":id,"serial":spec.serial,"configuration":spec.configuration,
                "installed":order.installed,"substitution_qualification":order.substitute.as_ref().map(|part|&setup.substitutes[part]),
                "work_cell":order.cell,"tests":order.tests,"repairs":order.repairs,"delivery":order.delivery});
            if setup.issued_report_inputs {
                link["material_inputs"] = json!(inputs);
            }
            Some(link)
        }).collect();
        return Ok((
            json!({"ok":true,"links":links,"test_bench":"TEST-1","occupied_by":state.bench,"queue":queue(&state,setup)}),
            false,
        ));
    }
    let id = args["work_order"].as_str().unwrap();
    let Some(order) = state.orders.get(id) else {
        return Ok((failure("WORK_ORDER_NOT_FOUND"), false));
    };
    if action == Action::ListOperationReports {
        let reports = match args["operation"].as_str().unwrap() {
            "build" => order.build_reports.as_slice(),
            "final_test" => &[],
            _ => return Ok((failure("OPERATION_OUT_OF_SEQUENCE"), false)),
        };
        return Ok((
            json!({"ok":true,"work_order":id,"operation":args["operation"],"reports":reports}),
            false,
        ));
    }
    if action == Action::AssemblyKit {
        let parts = required(setup, order);
        let shortages = if !has_build(order) {
            missing(setup, &parts, &state.stock)
        } else {
            BTreeMap::new()
        };
        return Ok((
            json!({"ok":true,"ready":shortages.is_empty(),"missing":shortages,"bom":parts,
            "stock":state.stock,"installed":order.installed,"configuration":setup.orders[id].configuration,"due_in_minutes":setup.orders[id].due_in_minutes}),
            false,
        ));
    }
    let key = args["idempotency_key"].as_str().unwrap();
    let request = json!({"action":action,"arguments":args}).to_string();
    let record: Option<(String, String)> = tx
        .query_row(
            "SELECT request,response FROM scenario_requests WHERE run=?1 AND key=?2",
            params![run, key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((original, response)) = record {
        return Ok((
            if original == request {
                serde_json::from_str(&response)?
            } else {
                failure("IDEMPOTENCY_CONFLICT")
            },
            false,
        ));
    }
    if matches!(
        action,
        Action::ReleaseWorkOrder
            | Action::HoldWorkOrder
            | Action::ResumeWorkOrder
            | Action::CancelWorkOrder
            | Action::CompleteWorkOrder
            | Action::CloseWorkOrder
            | Action::DispatchOperation
            | Action::ReassignOperation
            | Action::StartOperation
            | Action::PauseOperation
            | Action::ResumeOperation
            | Action::PostOperationReport
            | Action::ReverseOperationReport
            | Action::CompleteOperation
            | Action::SetDispatchPriority
    ) && args["expected_cursor"].as_i64() != Some(cursor)
    {
        return Ok((
            json!({"ok":false,"code":"VERSION_CONFLICT","cursor":cursor}),
            false,
        ));
    }
    if require_release
        && matches!(action, Action::AssemblyDispatch | Action::DispatchOperation)
        && order.phase == Phase::Planned
    {
        return Ok((failure("INVALID_STATE"), false));
    }
    let mut response = apply(
        &mut state,
        setup,
        action,
        id,
        args,
        cursor + 1,
        simulation,
        rules,
        model,
    );
    if response["ok"] != true {
        return Ok((response, false));
    }
    if setup.issued_report_inputs
        && action == Action::CancelWorkOrder
        && crate::mes::has_operation_issue(tx, run, id, "build", false)?
    {
        return Ok((failure("ORDER_HAS_EXECUTION"), false));
    }
    if setup.issued_report_inputs
        && matches!(action, Action::StartOperation | Action::PostOperationReport)
    {
        if let Some(code) = crate::quality::blocking(tx, run, id)? {
            return Ok((failure(&code), false));
        }
        if action == Action::StartOperation {
            if !crate::mes::has_operation_issue(tx, run, id, "build", true)? {
                return Ok((failure("MATERIAL_NOT_ISSUED"), false));
            }
        } else {
            let reports = &mut state.orders.get_mut(id).unwrap().build_reports;
            let inputs = match crate::mes::report_material_inputs(tx, run, reports, id, args)? {
                Ok(inputs) => inputs,
                Err(code) => return Ok((failure(code), false)),
            };
            if !crate::inventory::matches_build_requirements(tx, run, id, &inputs)? {
                return Ok((failure("MATERIAL_INPUTS_INCOMPLETE"), false));
            }
            let report = reports.last_mut().unwrap();
            report.material_inputs = inputs;
            response["report"] = json!(report);
        }
    }
    response["cursor"] = (cursor + 1).into();
    save(tx, run, &state)?;
    if action == Action::AssemblyStartTest {
        tx.execute("INSERT INTO des_runs VALUES (?1,?2) ON CONFLICT(run) DO UPDATE SET state=excluded.state",
            params![run,serde_json::to_string(simulation)?])?;
    }
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
    setup: &Setup,
    action: Action,
    id: &str,
    args: &Map<String, Value>,
    ticket: i64,
    simulation: &mut Simulation,
    rules: &[mom_des::Rule],
    model: &dyn mom_des::Model,
) -> Value {
    if state.orders[id].hold_reason.is_some()
        && !matches!(action, Action::ResumeWorkOrder | Action::CancelWorkOrder)
    {
        return failure("WORK_ORDER_HELD");
    }
    if action == Action::AssemblyStartTest {
        if state.bench.is_some() {
            return failure("TEST_BENCH_BUSY");
        }
        if queue(state, setup)
            .first()
            .is_some_and(|first| first.as_str() != id)
        {
            return failure("TEST_PRIORITY_BLOCKED");
        }
    }
    let busy = setup
        .exclusive_dispatch
        .then(|| {
            args.get("work_center")
                .or_else(|| args.get("cell"))
                .and_then(Value::as_str)
                .and_then(|center| cell_occupant(state, center, id))
                .map(str::to_owned)
        })
        .flatten();
    let reverse_busy = action == Action::ReverseOperationReport
        && state.orders[id].phase == Phase::Queued
        && state.orders[id]
            .cell
            .as_deref()
            .is_some_and(|cell| cell_occupant(state, cell, id).is_some());
    let order = state.orders.get_mut(id).unwrap();
    match action {
        Action::HoldWorkOrder
            if matches!(
                order.phase,
                Phase::Released | Phase::Dispatched | Phase::Queued
            ) =>
        {
            let reason = args["reason"].as_str().unwrap();
            order.hold_reason = Some(reason.into());
            return json!({"ok":true,"work_order":id,"status":"held","hold_reason":reason});
        }
        Action::ResumeWorkOrder if order.hold_reason.is_some() => {
            if matches!(order.phase, Phase::Released | Phase::Dispatched) {
                let shortages = missing(setup, &required(setup, order), &state.stock);
                if !shortages.is_empty() {
                    return json!({"ok":false,"code":"MATERIAL_SHORTAGE","missing":shortages});
                }
            }
            order.hold_reason = None;
            return json!({"ok":true,"work_order":id,"status":order.phase});
        }
        Action::CancelWorkOrder if !matches!(order.phase, Phase::Closed | Phase::Cancelled) => {
            if !matches!(
                order.phase,
                Phase::Planned | Phase::Released | Phase::Dispatched
            ) {
                return failure("ORDER_HAS_EXECUTION");
            }
            let reason = args["reason"].as_str().unwrap();
            order.phase = Phase::Cancelled;
            order.hold_reason = None;
            order.cancel_reason = Some(reason.into());
            return json!({"ok":true,"work_order":id,"status":"cancelled","cancel_reason":reason});
        }
        Action::CompleteWorkOrder | Action::CloseWorkOrder if order.phase == Phase::Failed => {
            return failure("QUALITY_HOLD");
        }
        Action::CompleteWorkOrder
            if matches!(order.phase, Phase::Passed | Phase::Delivered)
                && order.completion_confirmed.is_none() =>
        {
            order.completion_confirmed = Some(true);
            return json!({"ok":true,"work_order":id,"status":"completed"});
        }
        Action::CloseWorkOrder if matches!(order.phase, Phase::Passed | Phase::Delivered) => {
            if order.completion_confirmed.is_none() {
                return failure("COMPLETION_REQUIRED");
            }
            if order.phase != Phase::Delivered {
                return failure("DELIVERY_REQUIRED");
            }
            order.phase = Phase::Closed;
            return json!({"ok":true,"work_order":id,"status":"closed"});
        }
        Action::ReleaseWorkOrder => {
            let configuration = &setup.orders[id].configuration;
            let capable = setup
                .cells
                .values()
                .any(|configs| configs.contains(configuration));
            let parts = required(setup, order);
            let shortages = missing(setup, &parts, &state.stock);
            if let Some(code) = dispatch_blocker(order.phase == Phase::Planned, capable, &shortages)
            {
                return if code == "MATERIAL_SHORTAGE" {
                    json!({"ok":false,"code":code,"missing":shortages})
                } else {
                    failure(code)
                };
            }
            order.released_parts = Some(parts.clone());
            order.phase = Phase::Released;
            return json!({"ok":true,"work_order":id,"status":"released",
                "snapshot":{"configuration":configuration,"parts":parts,
                "substitution_qualification":order.substitute.as_ref().map(|part|&setup.substitutes[part])}});
        }
        Action::AssemblySubstitute
            if order.released_parts.is_none()
                && matches!(order.phase, Phase::Planned | Phase::Dispatched) =>
        {
            let part = args["part"].as_str().unwrap();
            let Some(qualification) = setup.substitutes.get(part) else {
                return failure("SUBSTITUTE_NOT_QUALIFIED");
            };
            order.substitute = Some(part.into());
            return json!({"ok":true,"part":part,"qualification":qualification});
        }
        Action::AssemblyDispatch | Action::DispatchOperation | Action::ReassignOperation => {
            if action != Action::AssemblyDispatch && args["operation"] != "build" {
                return failure(if args["operation"] == "final_test" {
                    "OPERATION_NOT_DISPATCHABLE"
                } else {
                    "OPERATION_NOT_FOUND"
                });
            }
            let Some(cell) = args[if action != Action::AssemblyDispatch {
                "work_center"
            } else {
                "cell"
            }]
            .as_str() else {
                return failure("INVALID_ARGUMENT");
            };
            let capable = setup.cells.get(cell).is_some_and(|configurations| {
                configurations.contains(&setup.orders[id].configuration)
            });
            let shortages = missing(setup, &required(setup, order), &state.stock);
            if let Some(code) = dispatch_blocker(
                if action == Action::ReassignOperation {
                    setup.exclusive_dispatch
                        && order.phase == Phase::Dispatched
                        && order.cell.as_deref() != Some(cell)
                } else {
                    matches!(order.phase, Phase::Planned | Phase::Released)
                },
                capable,
                &shortages,
            ) {
                return if code == "MATERIAL_SHORTAGE" {
                    json!({"ok":false,"code":code,"missing":shortages})
                } else {
                    failure(code)
                };
            }
            if let Some(owner) = busy {
                return json!({"ok":false,"code":"RESOURCE_BUSY","occupied_by":owner});
            }
            order.cell = Some(cell.into());
            order.phase = Phase::Dispatched;
        }
        Action::StartOperation | Action::ResumeOperation
            if args["operation"] == "build"
                && ((action == Action::StartOperation && order.phase == Phase::Dispatched)
                    || (action == Action::ResumeOperation
                        && order.phase == Phase::BuildPaused)) =>
        {
            let shortages = missing(setup, &required(setup, order), &state.stock);
            if !has_build(order) && !shortages.is_empty() {
                return json!({"ok":false,"code":"MATERIAL_SHORTAGE","missing":shortages});
            }
            order.phase = Phase::Building;
        }
        Action::PauseOperation
            if args["operation"] == "build" && order.phase == Phase::Building =>
        {
            order.phase = Phase::BuildPaused;
            order.build_pause_reason = Some(args["reason"].as_str().unwrap().into());
        }
        Action::AssemblyBuild | Action::PostOperationReport
            if (action == Action::AssemblyBuild
                && matches!(order.phase, Phase::Dispatched | Phase::Building))
                || (action == Action::PostOperationReport && order.phase == Phase::Building) =>
        {
            if action == Action::PostOperationReport {
                if args["operation"] != "build" {
                    return failure("OPERATION_OUT_OF_SEQUENCE");
                }
                let remaining = usize::from(!has_build(order));
                if args["quantity"].as_u64().unwrap() > remaining as u64 {
                    return json!({"ok":false,"code":"REPORT_EXCEEDS_REMAINING","remaining":remaining});
                }
            }
            let parts = required(setup, order);
            let shortages = missing(setup, &parts, &state.stock);
            if !shortages.is_empty() {
                return json!({"ok":false,"code":"MATERIAL_SHORTAGE","missing":shortages});
            }
            if !setup.issued_report_inputs {
                for (part, quantity) in &parts {
                    *state.stock.get_mut(part).unwrap() -= quantity;
                }
                order.installed = parts;
            }
            if action == Action::PostOperationReport {
                let serial = &setup.orders[id].serial;
                let report = crate::mes::Report {
                    id: format!("report-{ticket}"),
                    order: id.into(),
                    step: 0,
                    quantity: 1,
                    batch: None,
                    reversed: false,
                    serial_numbers: vec![serial.clone()],
                    components: order.installed.clone(),
                    material_inputs: Vec::new(),
                    material_input_serials: Default::default(),
                    rework: false,
                    wip_move: None,
                    wip_movement_reversals: Vec::new(),
                    wip_partitions: Vec::new(),
                    wip_inputs: Vec::new(),
                    wip_sources: Vec::new(),
                    consolidated_into: None,
                    coating: None,
                    tool_setup: None,
                };
                let response = json!({"ok":true,"report":report,"serial_numbers":[serial],"reported_quantity":1,"remaining":0});
                order.build_reports.push(report);
                return response;
            }
            order.phase = Phase::Queued;
        }
        Action::CompleteOperation if order.phase == Phase::Building => {
            if args["operation"] != "build" {
                return failure("OPERATION_OUT_OF_SEQUENCE");
            }
            if !has_build(order) {
                return json!({"ok":false,"code":"OPERATION_QUANTITY_INCOMPLETE","remaining":1});
            }
            order.phase = Phase::Queued;
            return json!({"ok":true,"work_order":id,"operation":"build","status":"completed"});
        }
        Action::ReverseOperationReport => {
            let Some(report) = order
                .build_reports
                .iter_mut()
                .find(|report| report.id == args["report_id"])
            else {
                return failure("REPORT_NOT_FOUND");
            };
            if report.reversed
                || !order.tests.is_empty()
                || !matches!(order.phase, Phase::Building | Phase::Queued)
            {
                return failure("REPORT_HAS_DEPENDENCIES");
            }
            if reverse_busy {
                return failure("RESOURCE_BUSY");
            }
            for (part, quantity) in std::mem::take(&mut order.installed) {
                *state.stock.get_mut(&part).unwrap() += quantity;
            }
            report.reversed = true;
            order.phase = Phase::Building;
            return json!({"ok":true,"work_order":id,"report_id":report.id,"reversed":true});
        }
        Action::AssemblyPriority | Action::SetDispatchPriority
            if matches!(
                order.phase,
                Phase::Planned | Phase::Released | Phase::Dispatched | Phase::Queued
            ) =>
        {
            order.priority = args["priority"].as_u64().unwrap();
            if action == Action::SetDispatchPriority {
                return json!({"ok":true,"work_order":id,"priority":order.priority});
            }
        }
        Action::AssemblyStartTest if order.phase == Phase::Queued => {
            let command = Command::Intervene {
                action: format!("assembly_test:{ticket}"),
            };
            if let Err(code) = simulation.execute(command, rules, model) {
                return failure(code);
            }
            state.bench = Some(id.into());
            order.phase = Phase::Testing;
            order.tests.push(Test {
                ticket,
                start_minute: simulation.timeline.clock,
                end_minute: None,
                result: None,
            });
        }
        Action::AssemblyFinishTest if order.phase == Phase::Testing => {
            let first = order.tests.len() == 1;
            let test = order.tests.last_mut().unwrap();
            let events = &simulation.timeline.events;
            let Some(completion) = events.iter().find(|event| {
                event.kind == "assembly_test_complete" && event.payload["ticket"] == test.ticket
            }) else {
                return failure("TEST_NOT_READY");
            };
            let failed = first
                && events.iter().any(|event| {
                    event.cursor <= completion.cursor
                        && event.kind == "assembly_test_fault"
                        && event.payload["work_order"] == id
                });
            test.end_minute = Some(completion.minute);
            test.result = Some(if failed { "fail" } else { "pass" }.into());
            order.phase = if failed { Phase::Failed } else { Phase::Passed };
            state.bench = None;
        }
        Action::AssemblyRepair if order.phase == Phase::Failed => {
            order.repairs.push(args["reason"].as_str().unwrap().into());
            order.phase = Phase::Queued;
        }
        Action::AssemblyDeliver if order.phase == Phase::Passed => {
            order.delivery = Some(
                json!({"status":"delivered","serial":setup.orders[id].serial,"minute":simulation.timeline.clock}),
            );
            order.phase = Phase::Delivered;
        }
        Action::AssemblyDeliver => return failure("TEST_PASS_REQUIRED"),
        _ => return failure("INVALID_STATE"),
    }
    json!({"ok":true,"work_order":id,"serial":setup.orders[id].serial,"state":order,"test_bench":"TEST-1",
        "occupied_by":state.bench,"queue":queue(state,setup)})
}
