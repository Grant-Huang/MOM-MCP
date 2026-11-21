use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    catalog::{self, Action},
    simulation::{self, Command, Simulation},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    scenario_id: String,
    version: u32,
    title: String,
    model: String,
    #[serde(default)]
    manufacturing: Option<crate::mes::Setup>,
    #[serde(default)]
    smt: Option<crate::smt::Setup>,
    #[serde(default)]
    assembly: Option<crate::assembly::Setup>,
    #[serde(default = "model_version")]
    model_version: u32,
    #[serde(default)]
    model_parameters: Map<String, Value>,
    #[serde(default)]
    event_rules: Vec<mom_des::Rule>,
    work_orders: Vec<String>,
    materials: BTreeMap<String, String>,
    capabilities: Vec<Action>,
}

const fn model_version() -> u32 {
    1
}

#[derive(Clone)]
pub struct Scenarios {
    db: PathBuf,
}

pub struct Rejection {
    code: &'static str,
    field: String,
}

impl Rejection {
    fn json(self) -> Value {
        json!({"ok":false,"code":self.code,"field":self.field})
    }
}

fn reject(code: &'static str, field: &'static str) -> Rejection {
    Rejection {
        code,
        field: field.into(),
    }
}

impl Rejection {
    pub fn invalid(field: &str) -> Self {
        Self {
            code: "INVALID_ARGUMENT",
            field: field.into(),
        }
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn explicit_time(value: &Value, path: &str) -> Option<String> {
    match value {
        Value::Object(fields) => fields.iter().find_map(|(name, value)| {
            let key = name.replace(['_', '-'], "").to_ascii_lowercase();
            let field = format!("{path}.{name}");
            if matches!(
                key.as_str(),
                "occurredat"
                    | "eventtime"
                    | "atminute"
                    | "timestamp"
                    | "timeline"
                    | "startminute"
                    | "scheduledat"
                    | "dueat"
            ) {
                Some(field)
            } else {
                explicit_time(value, &field)
            }
        }),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .find_map(|(index, value)| explicit_time(value, &format!("{path}[{index}]"))),
        _ => None,
    }
}

fn parse_definition(source: &str) -> anyhow::Result<Definition> {
    let raw: Value = serde_json::from_str(source)?;
    if let Some(field) = explicit_time(&raw, "$") {
        anyhow::bail!("explicit event timestamps are not supported at {field}");
    }
    let definition: Definition = serde_json::from_value(raw)?;
    anyhow::ensure!(identifier(&definition.scenario_id), "invalid scenario_id");
    anyhow::ensure!(definition.version > 0, "invalid version");
    anyhow::ensure!(
        !definition.title.trim().is_empty() && definition.title.len() <= 160,
        "invalid title"
    );
    anyhow::ensure!(identifier(&definition.model), "invalid model");
    anyhow::ensure!(definition.event_rules.len() <= 64, "too many event_rules");
    for (index, rule) in definition.event_rules.iter().enumerate() {
        anyhow::ensure!(
            identifier(&rule.event),
            "invalid event_rules[{index}].event"
        );
        anyhow::ensure!(rule.delay.valid(), "invalid event_rules[{index}].delay");
    }
    let orders = &definition.work_orders;
    anyhow::ensure!(
        !orders.is_empty() && orders.iter().all(|id| identifier(id)),
        "invalid work_orders"
    );
    anyhow::ensure!(
        orders
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == orders.len(),
        "duplicate work_orders"
    );
    for (batch, order) in &definition.materials {
        anyhow::ensure!(identifier(batch), "invalid materials batch {batch}");
        anyhow::ensure!(
            orders.contains(order),
            "materials.{batch} references unknown work order {order}"
        );
    }
    anyhow::ensure!(
        definition
            .capabilities
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == definition.capabilities.len(),
        "duplicate capabilities"
    );
    if let Some(setup) = &definition.manufacturing {
        anyhow::ensure!(
            setup.initial_stock <= 1_000_000,
            "manufacturing.initial_stock exceeds the supported range"
        );
        anyhow::ensure!(
            definition.materials.contains_key("LOT-1"),
            "manufacturing requires materials.LOT-1"
        );
    }
    simulation::load_model(
        &definition.model,
        definition.model_version,
        &definition.model_parameters,
    )
    .map_err(|reason| anyhow::anyhow!("model configuration: {reason}"))?;
    if let Some(setup) = &definition.smt {
        setup.validate(
            &definition.work_orders,
            &definition.model,
            &definition.model_parameters,
        )?;
    }
    anyhow::ensure!(
        definition.smt.is_some()
            || !definition
                .capabilities
                .iter()
                .any(|action| crate::smt::TOOLS.iter().any(|tool| tool.0 == *action)),
        "SMT capabilities require smt setup"
    );
    if let Some(setup) = &definition.assembly {
        setup.validate(
            &definition.work_orders,
            &definition.model,
            &definition.model_parameters,
        )?;
    }
    anyhow::ensure!(
        definition.assembly.is_some()
            || !definition
                .capabilities
                .iter()
                .any(|action| crate::assembly::TOOLS.iter().any(|tool| tool.0 == *action)),
        "assembly capabilities require assembly setup"
    );
    Ok(definition)
}

impl Scenarios {
    pub fn new(db: PathBuf) -> anyhow::Result<Self> {
        let store = Self { db };
        let conn = store.connect()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS scenario_definitions (
                 id TEXT NOT NULL, version INTEGER NOT NULL, definition TEXT NOT NULL,
                 PRIMARY KEY (id, version)
             );
             CREATE TABLE IF NOT EXISTS scenario_runs (
                 id TEXT PRIMARY KEY, scenario TEXT NOT NULL, version INTEGER NOT NULL,
                 seed TEXT NOT NULL, closed INTEGER NOT NULL DEFAULT 0, cursor INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS scenario_holds (
                 run TEXT NOT NULL, batch TEXT NOT NULL, reason TEXT NOT NULL,
                 PRIMARY KEY (run, batch)
             );
             CREATE TABLE IF NOT EXISTS assembly_runs (run TEXT PRIMARY KEY, state TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS smt_runs (run TEXT PRIMARY KEY, state TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS mes_runs (run TEXT PRIMARY KEY, state TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS des_runs (run TEXT PRIMARY KEY, state TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS scenario_requests (
                 run TEXT NOT NULL, key TEXT NOT NULL, request TEXT NOT NULL, response TEXT NOT NULL,
                 PRIMARY KEY (run, key)
             );"
        )?;
        for source in [
            include_str!("../fixtures/cnc.json"),
            include_str!("../fixtures/cnc-manufacturing.json"),
            include_str!("../fixtures/cnc-shortage.json"),
            include_str!("../fixtures/cnc-lot.json"),
            include_str!("../fixtures/smt.json"),
            include_str!("../fixtures/smt-production.json"),
            include_str!("../fixtures/assembly.json"),
            include_str!("../fixtures/assembly-production.json"),
            include_str!("../fixtures/assembly-shortage.json"),
        ] {
            let definition = parse_definition(source)?;
            conn.execute(
                "INSERT OR IGNORE INTO scenario_definitions VALUES (?1,?2,?3)",
                params![definition.scenario_id, definition.version, source],
            )?;
        }
        Ok(store)
    }

    fn connect(&self) -> rusqlite::Result<Connection> {
        let connection = Connection::open(&self.db)?;
        connection.busy_timeout(Duration::from_secs(2))?;
        Ok(connection)
    }

    pub fn register(&self, source: &Path) -> anyhow::Result<()> {
        let definition = parse_definition(&std::fs::read_to_string(source)?)?;
        self.connect()?.execute(
            "INSERT INTO scenario_definitions VALUES (?1,?2,?3)",
            params![
                definition.scenario_id,
                definition.version,
                serde_json::to_string(&definition)?
            ],
        )?;
        Ok(())
    }

    pub fn invoke(
        &self,
        name: &str,
        args: Map<String, Value>,
        control: bool,
    ) -> anyhow::Result<Value> {
        let Some(tool) = catalog::gateway(name).filter(|tool| control || !tool.control) else {
            return Ok(reject("UNKNOWN_TOOL", "name").json());
        };
        if let Err(error) = catalog::validate(&tool.input, &args) {
            return Ok(error.json());
        }
        match self.dispatch(name, &args)? {
            Ok(value) => Ok(value),
            Err(error) => Ok(error.json()),
        }
    }

    fn dispatch(
        &self,
        name: &str,
        args: &Map<String, Value>,
    ) -> anyhow::Result<Result<Value, Rejection>> {
        let mut conn = self.connect()?;
        if name == "list_scenarios" {
            let mut query =
                conn.prepare("SELECT definition FROM scenario_definitions ORDER BY id, version")?;
            let mut scenarios = Vec::new();
            for row in query.query_map([], |r| r.get::<_, String>(0))? {
                let spec: Definition = serde_json::from_str(&row?)?;
                scenarios.push(json!({"scenario_id":spec.scenario_id,"version":spec.version,"title":spec.title,"model":spec.model,"model_version":spec.model_version,"capabilities":spec.capabilities,"mode":"mock","run_parameters":{"seed":{"type":"integer","minimum":0,"default":1}}}));
            }
            return Ok(Ok(json!({"ok":true,"scenarios":scenarios})));
        }
        if name == "open_scenario" {
            let scenario = args["scenario_id"].as_str().unwrap();
            let version = args["version"].as_i64().unwrap();
            let seed = args
                .get("seed")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .to_string();
            let source: Option<String> = conn
                .query_row(
                    "SELECT definition FROM scenario_definitions WHERE id=?1 AND version=?2",
                    params![scenario, version],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(source) = source else {
                return Ok(Err(reject("SCENARIO_NOT_FOUND", "scenario_id")));
            };
            // P1 allowed definitions before the versioned registry was available.
            let definition: Definition = serde_json::from_str(&source)?;
            if simulation::load_model(
                &definition.model,
                definition.model_version,
                &definition.model_parameters,
            )
            .is_err()
            {
                return Ok(Err(reject("MODEL_CONFIGURATION_INVALID", "model")));
            }
            let id: String = conn.query_row(
                "INSERT INTO scenario_runs (id,scenario,version,seed)
                 VALUES (lower(hex(randomblob(16))),?1,?2,?3) RETURNING id",
                params![scenario, version, seed],
                |row| row.get(0),
            )?;
            return Ok(Ok(run_view(&id, scenario, version, &seed, false, 0, None)));
        }

        let run = args["run_id"].as_str().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = tx
            .query_row(
                "SELECT scenario,version,seed,closed,cursor FROM scenario_runs WHERE id=?1",
                [run],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, bool>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((scenario, version, seed, closed, cursor)) = record else {
            return Ok(Err(reject("RUN_NOT_FOUND", "run_id")));
        };
        let serialized: Option<String> = tx
            .query_row("SELECT state FROM des_runs WHERE run=?1", [run], |row| {
                row.get(0)
            })
            .optional()?;
        let stored: Option<Simulation> = serialized
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        if name == "get_run" {
            return Ok(Ok(run_view(
                run,
                &scenario,
                version,
                &seed,
                closed,
                cursor,
                stored.as_ref(),
            )));
        }
        if name == "close_run" {
            tx.execute("UPDATE scenario_runs SET closed=1 WHERE id=?1", [run])?;
            tx.commit()?;
            return Ok(Ok(run_view(
                run,
                &scenario,
                version,
                &seed,
                true,
                cursor,
                stored.as_ref(),
            )));
        }
        if name == "read_events" {
            let after = args.get("after").and_then(Value::as_u64).unwrap_or(0) as usize;
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
            let total = stored
                .as_ref()
                .map_or(0, |state| state.timeline.events.len());
            if after > total {
                return Ok(Err(reject("INVALID_CURSOR", "after")));
            }
            return Ok(Ok(stored.as_ref().map_or_else(
                || json!({"ok":true,"clock":0,"events":[],"event_cursor":0,"has_more":false}),
                |state| state.page(after, limit),
            )));
        }
        if closed {
            return Ok(Err(reject("RUN_CLOSED", "run_id")));
        }
        let source: String = tx.query_row(
            "SELECT definition FROM scenario_definitions WHERE id=?1 AND version=?2",
            params![scenario, version],
            |row| row.get(0),
        )?;
        let definition: Definition = serde_json::from_str(&source)?;
        if matches!(name, "advance_run" | "intervene_run" | "inspect_run") {
            let model = simulation::load_model(
                &definition.model,
                definition.model_version,
                &definition.model_parameters,
            )
            .map_err(|reason| anyhow::anyhow!("{reason}"))?;
            let mut state = stored.unwrap_or_else(|| {
                Simulation::new(
                    seed.parse().expect("stored seed"),
                    &definition.event_rules,
                    model.as_ref(),
                )
            });
            if name == "inspect_run" {
                let replay_verified =
                    state.verify_replay(seed.parse()?, &definition.event_rules, model.as_ref())?;
                return Ok(Ok(
                    json!({"ok":true,"seed":seed,"definition":definition,"model":definition.model,
                    "model_version":definition.model_version,"engine_version":env!("CARGO_PKG_VERSION"),
                    "diagnosis":model.diagnosis(),"replay_verified":replay_verified,"replay":state}),
                ));
            }

            let command = if name == "advance_run" {
                Command::Advance {
                    event_budget: args.get("max_events").and_then(Value::as_u64).unwrap_or(1)
                        as usize,
                    minutes: args.get("max_minutes").and_then(Value::as_u64),
                }
            } else {
                Command::Intervene {
                    action: args["action"].as_str().expect("validated action").into(),
                }
            };
            if matches!(&command, Command::Intervene { action } if action.starts_with("assembly_test:"))
            {
                return Ok(Err(reject("INTERVENTION_UNSUPPORTED", "action")));
            }
            let stop = match state.execute(command, &definition.event_rules, model.as_ref()) {
                Ok(stop) => stop,
                Err(code) => {
                    return Ok(Err(reject(
                        code,
                        if name == "advance_run" {
                            "run_id"
                        } else {
                            "action"
                        },
                    )));
                }
            };
            let response = json!({"ok":true,"clock":state.timeline.clock,
                "event_cursor":state.timeline.events.len(),"stop":stop});
            tx.execute(
                "INSERT INTO des_runs (run,state) VALUES (?1,?2)
                ON CONFLICT(run) DO UPDATE SET state=excluded.state",
                params![run, serde_json::to_string(&state)?],
            )?;
            if let Some(setup) = &definition.manufacturing {
                crate::mes::project_receipts(
                    &tx,
                    run,
                    setup,
                    &definition.work_orders,
                    &state.timeline.events,
                )?;
            }
            if let Some(setup) = &definition.assembly {
                crate::assembly::project_receipt(&tx, run, setup, &state.timeline.events)?;
            }
            tx.commit()?;
            return Ok(Ok(response));
        }
        if name == "search_tools" {
            return Ok(Ok(
                json!({"ok":true,"tools":catalog::search(&definition.capabilities, args)}),
            ));
        }
        let action = args["name"].as_str().unwrap();
        let Some(action) = Action::named(action) else {
            return Ok(Err(reject("UNKNOWN_TOOL", "name")));
        };
        if !definition.capabilities.contains(&action) {
            return Ok(Err(reject("TOOL_UNAVAILABLE", "name")));
        }
        let spec = action.spec();
        if name == "describe_tool" {
            return Ok(Ok(json!({"ok":true,"tool":spec})));
        }
        let parameters = args["arguments"].as_object().unwrap();
        if let Err(error) = catalog::validate(&spec.input, parameters) {
            return Ok(Err(error));
        }
        if crate::assembly::TOOLS.iter().any(|tool| tool.0 == action) {
            let Some(setup) = &definition.assembly else {
                return Ok(Err(reject("TOOL_UNAVAILABLE", "name")));
            };
            let model = simulation::load_model(
                &definition.model,
                definition.model_version,
                &definition.model_parameters,
            )
            .map_err(|reason| anyhow::anyhow!("{reason}"))?;
            let mut simulation = stored.unwrap_or_else(|| {
                Simulation::new(
                    seed.parse().expect("stored seed"),
                    &definition.event_rules,
                    model.as_ref(),
                )
            });
            let (response, changed) = crate::assembly::invoke(
                &tx,
                run,
                setup,
                action,
                parameters,
                cursor,
                &mut simulation,
                &definition.event_rules,
                model.as_ref(),
            )?;
            if changed {
                tx.commit()?;
            }
            return Ok(Ok(response));
        }
        if crate::smt::TOOLS.iter().any(|tool| tool.0 == action) {
            let Some(setup) = &definition.smt else {
                return Ok(Err(reject("TOOL_UNAVAILABLE", "name")));
            };
            let events = stored
                .as_ref()
                .map_or(&[][..], |state| state.timeline.events.as_slice());
            let (response, changed) =
                crate::smt::invoke(&tx, run, setup, action, parameters, cursor, events)?;
            if changed {
                tx.commit()?;
            }
            return Ok(Ok(response));
        }
        if crate::mes::TOOLS.iter().any(|tool| tool.0 == action) {
            let Some(setup) = &definition.manufacturing else {
                return Ok(Err(reject("TOOL_UNAVAILABLE", "name")));
            };
            let (response, changed) = crate::mes::invoke(
                &tx,
                run,
                setup,
                &definition.work_orders,
                action,
                parameters,
                cursor,
            )?;
            if changed {
                tx.commit()?;
            }
            return Ok(Ok(response));
        }
        if action == Action::GetWorkOrder {
            let order = parameters["work_order"].as_str().unwrap();
            if let Some(setup) = &definition.assembly {
                return Ok(crate::assembly::status(&tx, run, setup, order)?
                    .ok_or_else(|| reject("WORK_ORDER_NOT_FOUND", "work_order")));
            }
            if let Some(setup) = &definition.smt {
                if !definition.work_orders.iter().any(|id| id == order) {
                    return Ok(Err(reject("WORK_ORDER_NOT_FOUND", "work_order")));
                }
                let status = crate::smt::work_order_status(&tx, run, setup, order)?;
                return Ok(Ok(
                    json!({"ok":true,"run_id":run,"work_order":order,"status":status}),
                ));
            }
            if let Some(setup) = &definition.manufacturing {
                return Ok(crate::mes::order_status(
                    &tx,
                    run,
                    setup,
                    &definition.work_orders,
                    order,
                )?
                .ok_or_else(|| reject("WORK_ORDER_NOT_FOUND", "work_order")));
            }
            return Ok(
                if definition
                    .work_orders
                    .iter()
                    .any(|candidate| candidate == order)
                {
                    Ok(json!({"ok":true,"run_id":run,"work_order":order,"status":"planned"}))
                } else {
                    Err(reject("WORK_ORDER_NOT_FOUND", "work_order"))
                },
            );
        }
        let batch = parameters["batch"].as_str().unwrap();
        let Some(order) = definition.materials.get(batch) else {
            return Ok(Err(reject("MATERIAL_NOT_FOUND", "batch")));
        };
        let held: Option<String> = tx
            .query_row(
                "SELECT reason FROM scenario_holds WHERE run=?1 AND batch=?2",
                params![run, batch],
                |r| r.get(0),
            )
            .optional()?;
        if action == Action::GetMaterial {
            return Ok(Ok(
                json!({"ok":true,"run_id":run,"batch":batch,"work_order":order,"held":held.is_some(),"reason":held,"cursor":cursor}),
            ));
        }
        let reason = parameters["reason"].as_str().unwrap();
        let key = parameters["idempotency_key"].as_str().unwrap();
        let request = serde_json::to_string(parameters)?;
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
        if held.is_some() {
            return Ok(Err(reject("ALREADY_HELD", "batch")));
        }
        tx.execute(
            "INSERT INTO scenario_holds VALUES (?1,?2,?3)",
            params![run, batch, reason],
        )?;
        tx.execute(
            "UPDATE scenario_runs SET cursor=cursor+1 WHERE id=?1",
            [run],
        )?;
        let response = json!({"ok":true,"run_id":run,"batch":batch,"held":true,"reason":reason,"cursor":cursor+1});
        tx.execute(
            "INSERT INTO scenario_requests VALUES (?1,?2,?3,?4)",
            params![run, key, request, response.to_string()],
        )?;
        tx.commit()?;
        Ok(Ok(response))
    }
}

fn run_view(
    run: &str,
    scenario: &str,
    version: i64,
    seed: &str,
    closed: bool,
    cursor: i64,
    state: Option<&Simulation>,
) -> Value {
    json!({"ok":true,"run_id":run,"scenario_id":scenario,"version":version,"seed":seed.parse::<u64>().expect("stored seed"),"closed":closed,"clock":state.map_or(0, |state| state.timeline.clock),"event_cursor":state.map_or(0, |state| state.timeline.events.len()),"cursor":cursor,"mode":"mock"})
}
