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
    phase: Phase,
    substitute: Option<String>,
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
    Dispatched,
    Queued,
    Testing,
    Failed,
    Passed,
    Delivered,
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

fn load(tx: &Transaction<'_>, run: &str, setup: &Setup) -> anyhow::Result<State> {
    let stored: Option<String> = tx
        .query_row(
            "SELECT state FROM assembly_runs WHERE run=?1",
            [run],
            |row| row.get(0),
        )
        .optional()?;
    Ok(stored.map_or_else(
        || {
            Ok(State {
                stock: setup.stock.clone(),
                orders: setup
                    .orders
                    .keys()
                    .map(|id| (id.clone(), Order::default()))
                    .collect(),
                bench: None,
                received: false,
            })
        },
        |value| serde_json::from_str(&value),
    )?)
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
    let mut required = setup.bom.clone();
    if let Some(part) = &order.substitute {
        let quantity = required.remove("DRIVE-STD").unwrap();
        required.insert(part.clone(), quantity);
    }
    required
}

fn missing(
    required: &BTreeMap<String, usize>,
    stock: &BTreeMap<String, usize>,
) -> BTreeMap<String, usize> {
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
        .filter(|(_, order)| order.phase == Phase::Queued)
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
    Ok(state
        .orders
        .get(id)
        .map(|order| json!({"ok":true,"run_id":run,"work_order":id,"status":order.phase})))
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
    let shortages = missing(&required(setup, order), &state.stock);
    let result = dispatch_blocker(order, !cells.is_empty(), &shortages);
    Ok(json!({"ok":true,"work_order":id,"ready":result.is_none(),
        "blocking":result,"cursor":cursor,"eligible_cells":cells,"missing":shortages}))
}

fn dispatch_blocker(
    order: &Order,
    capable: bool,
    shortages: &BTreeMap<String, usize>,
) -> Option<&'static str> {
    if order.phase != Phase::Planned {
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
    simulation: &mut Simulation,
    rules: &[mom_des::Rule],
    model: &dyn mom_des::Model,
) -> anyhow::Result<(Value, bool)> {
    let mut state = load(tx, run, setup)?;
    if action == Action::AssemblyTrace {
        let subject = args["subject"].as_str().unwrap();
        let links:Vec<_>=state.orders.iter().filter_map(|(id,order)| {
            let spec=&setup.orders[id];
            (subject==id || subject==spec.serial || order.installed.contains_key(subject)).then(||
                json!({"work_order":id,"serial":spec.serial,"configuration":spec.configuration,
                    "installed":order.installed,"substitution_qualification":order.substitute.as_ref().map(|part|&setup.substitutes[part]),
                    "work_cell":order.cell,"tests":order.tests,"repairs":order.repairs,"delivery":order.delivery}))
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
    if action == Action::AssemblyKit {
        let parts = required(setup, order);
        let shortages = if order.installed.is_empty() {
            missing(&parts, &state.stock)
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
    let order = state.orders.get_mut(id).unwrap();
    match action {
        Action::AssemblySubstitute if matches!(order.phase, Phase::Planned | Phase::Dispatched) => {
            let part = args["part"].as_str().unwrap();
            let Some(qualification) = setup.substitutes.get(part) else {
                return failure("SUBSTITUTE_NOT_QUALIFIED");
            };
            order.substitute = Some(part.into());
            return json!({"ok":true,"part":part,"qualification":qualification});
        }
        Action::AssemblyDispatch => {
            let cell = args["cell"].as_str().unwrap();
            let capable = setup.cells.get(cell).is_some_and(|configurations| {
                configurations.contains(&setup.orders[id].configuration)
            });
            let shortages = missing(&required(setup, order), &state.stock);
            if let Some(code) = dispatch_blocker(order, capable, &shortages) {
                return if code == "MATERIAL_SHORTAGE" {
                    json!({"ok":false,"code":code,"missing":shortages})
                } else {
                    failure(code)
                };
            }
            order.cell = Some(cell.into());
            order.phase = Phase::Dispatched;
        }
        Action::AssemblyBuild if order.phase == Phase::Dispatched => {
            let parts = required(setup, order);
            let shortages = missing(&parts, &state.stock);
            if !shortages.is_empty() {
                return json!({"ok":false,"code":"MATERIAL_SHORTAGE","missing":shortages});
            }
            for (part, quantity) in &parts {
                *state.stock.get_mut(part).unwrap() -= quantity;
            }
            order.installed = parts;
            order.phase = Phase::Queued;
        }
        Action::AssemblyPriority
            if matches!(
                order.phase,
                Phase::Planned | Phase::Dispatched | Phase::Queued
            ) =>
        {
            order.priority = args["priority"].as_u64().unwrap();
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
