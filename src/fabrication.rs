mod nesting;

use std::collections::BTreeMap;

use mom_des::Event;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::catalog::{Action, Spec, object, text};

pub const SHEET: &[(Action, &str, &str, &str)] = &[
    (
        Action::SheetIssue,
        "sheet_issue",
        "发料及余料选用",
        "inventory",
    ),
    (Action::SheetNest, "sheet_nest", "按面积套料", "production"),
    (
        Action::SheetCut,
        "sheet_cut",
        "切割并记录用料、零件、废料",
        "production",
    ),
    (Action::SheetForm, "sheet_form", "成形零件", "production"),
    (Action::SheetInspect, "sheet_inspect", "钣金检验", "quality"),
    (
        Action::SheetReturn,
        "sheet_return",
        "将余料回库",
        "inventory",
    ),
    (
        Action::SheetTrace,
        "sheet_trace",
        "查询钣金面积台账与批次谱系",
        "inventory",
    ),
];

pub const HARNESS: &[(Action, &str, &str, &str)] = &[
    (
        Action::HarnessCut,
        "harness_cut",
        "按序列号裁线",
        "production",
    ),
    (
        Action::HarnessCrimp,
        "harness_crimp",
        "压接并校验替代端子资格",
        "production",
    ),
    (
        Action::HarnessSample,
        "harness_sample",
        "按抽检失败事件隔离线束",
        "quality",
    ),
    (
        Action::HarnessTest,
        "harness_test",
        "导通测试或复测",
        "quality",
    ),
    (
        Action::HarnessRework,
        "harness_rework",
        "更换端子并登记返工工艺",
        "quality",
    ),
    (
        Action::HarnessRelease,
        "harness_release",
        "放行合格线束",
        "production",
    ),
    (
        Action::HarnessTrace,
        "harness_trace",
        "按线束或端子批次查实际谱系",
        "inventory",
    ),
];

pub fn tools() -> impl Iterator<Item = &'static (Action, &'static str, &'static str, &'static str)>
{
    SHEET.iter().chain(HARNESS).chain(nesting::TOOLS)
}

pub fn spec(action: Action) -> Spec {
    if nesting::TOOLS.iter().any(|tool| tool.0 == action) {
        return nesting::spec(action);
    }
    let &(_, name, description, domain) = tools().find(|item| item.0 == action).unwrap();
    let mut required = match action {
        Action::SheetIssue => vec!["stock"],
        Action::SheetCut => vec!["machine"],
        Action::SheetInspect => vec!["result"],
        Action::SheetTrace | Action::HarnessTrace => vec!["subject"],
        Action::HarnessSample => vec!["event_cursor"],
        Action::HarnessCut | Action::HarnessRelease => vec!["serial"],
        Action::HarnessCrimp => vec!["serial", "terminal_batch"],
        Action::HarnessRework => vec!["serial", "terminal_batch", "reason"],
        Action::HarnessTest => vec!["serial", "result"],
        _ => vec![],
    };
    let read = matches!(action, Action::SheetTrace | Action::HarnessTrace);
    if !read {
        required.push("idempotency_key");
    }
    let fields = required
        .iter()
        .map(|&field| {
            (
                field.into(),
                match field {
                    "result" => json!({"type":"string","enum":["pass","fail"]}),
                    "event_cursor" => json!({"type":"integer","minimum":1,"maximum":10000}),
                    _ => text(),
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
            "update_fabrication_state"
        },
        errors: &[
            "INVALID_STATE",
            "STOCK_UNAVAILABLE",
            "STOCK_INELIGIBLE",
            "INSUFFICIENT_AREA",
            "MACHINE_UNAVAILABLE",
            "QUALITY_HOLD",
            "TEST_PASS_REQUIRED",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Setup {
    SheetMetal,
    SheetNesting,
    SheetDimensionalNesting,
    SheetRemnantNesting,
    SheetLayoutNesting,
    WireHarness,
}

impl Setup {
    fn supports_remnants(self) -> bool {
        matches!(self, Self::SheetRemnantNesting | Self::SheetLayoutNesting)
    }
    pub fn valid(self, model: &str, action: Action) -> bool {
        self.model() == model
            && (!matches!(
                action,
                Action::ReturnSheetRemnant | Action::TraceSheetMaterial
            ) || self.supports_remnants())
            && match self {
                Self::SheetMetal => SHEET,
                Self::SheetNesting
                | Self::SheetDimensionalNesting
                | Self::SheetRemnantNesting
                | Self::SheetLayoutNesting => nesting::TOOLS,
                Self::WireHarness => HARNESS,
            }
            .iter()
            .any(|tool| tool.0 == action)
    }

    pub fn model(self) -> &'static str {
        match self {
            Self::SheetMetal
            | Self::SheetNesting
            | Self::SheetDimensionalNesting
            | Self::SheetRemnantNesting
            | Self::SheetLayoutNesting => mom_model_fabrication::SHEET,
            Self::WireHarness => mom_model_fabrication::HARNESS,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Stock {
    lot: String,
    alloy: String,
    area_mm2: u64,
    eligible: bool,
    parent: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Sheet {
    inventory: BTreeMap<String, Stock>,
    issued: Option<String>,
    phase: SheetPhase,
    machine: Option<String>,
    ledger: Vec<Value>,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SheetPhase {
    Planned,
    Issued,
    Nested,
    Cut,
    Formed,
    Failed,
    Inspected,
    Complete,
}

impl Default for Sheet {
    fn default() -> Self {
        let inventory = [
            ("SHEET-FULL", "LOT-AL-1", "AL-5052", 1800, true, None),
            (
                "REM-OK",
                "LOT-AL-2",
                "AL-5052",
                1000,
                true,
                Some("SHEET-AL-2"),
            ),
            (
                "REM-SMALL",
                "LOT-AL-2",
                "AL-5052",
                650,
                true,
                Some("SHEET-AL-2"),
            ),
            (
                "REM-BAD",
                "LOT-STEEL",
                "SS-304",
                1500,
                false,
                Some("SHEET-STEEL"),
            ),
        ]
        .into_iter()
        .map(|(id, lot, alloy, area_mm2, eligible, parent)| {
            (
                id.into(),
                Stock {
                    lot: lot.into(),
                    alloy: alloy.into(),
                    area_mm2,
                    eligible,
                    parent: parent.map(str::to_owned),
                },
            )
        })
        .collect();
        Self {
            inventory,
            issued: None,
            phase: SheetPhase::Planned,
            machine: None,
            ledger: vec![],
        }
    }
}

const PART_AREA: u64 = 600;
const SCRAP_AREA: u64 = 100;
const CUT_AREA: u64 = PART_AREA + SCRAP_AREA;

fn error(code: &str) -> Value {
    json!({"ok":false,"code":code})
}
fn event_matching(events: &[Event], kind: &str, field: &str, id: &str) -> bool {
    events
        .iter()
        .any(|event| event.kind == kind && event.payload[field] == id)
}

impl Sheet {
    fn apply(&mut self, action: Action, args: &Map<String, Value>, events: &[Event]) -> Value {
        match action {
            Action::SheetTrace => {
                let subject = args["subject"].as_str().unwrap();
                let stock: BTreeMap<_, _> = self
                    .inventory
                    .iter()
                    .filter(|(id, stock)| {
                        subject == "WO-SHEET"
                            || subject == id.as_str()
                            || subject == stock.lot
                            || stock.parent.as_deref() == Some(subject)
                    })
                    .collect();
                let ledger: Vec<_> = self
                    .ledger
                    .iter()
                    .filter(|entry| {
                        subject == "WO-SHEET"
                            || entry["lot"] == subject
                            || entry["stock"] == subject
                            || entry["parent"] == subject
                    })
                    .collect();
                return json!({"ok":true,"phase":self.phase,"issued":self.issued,"machine":self.machine,
                    "area_unit":"mm2","stock":stock,"ledger":ledger});
            }
            Action::SheetIssue
                if matches!(
                    self.phase,
                    SheetPhase::Planned | SheetPhase::Issued | SheetPhase::Nested
                ) =>
            {
                let id = args["stock"].as_str().unwrap();
                let Some(stock) = self.inventory.get(id) else {
                    return error("STOCK_NOT_FOUND");
                };
                if event_matching(events, "sheet_supply_shortage", "stock", id) {
                    return error("STOCK_UNAVAILABLE");
                }
                if !stock.eligible || stock.alloy != "AL-5052" {
                    return error("STOCK_INELIGIBLE");
                }
                if stock.area_mm2 < CUT_AREA {
                    return error("INSUFFICIENT_AREA");
                }
                self.issued = Some(id.into());
                self.phase = SheetPhase::Issued;
            }
            Action::SheetNest if self.phase == SheetPhase::Issued => {
                self.phase = SheetPhase::Nested
            }
            Action::SheetCut if self.phase == SheetPhase::Nested => {
                let machine = args["machine"].as_str().unwrap();
                if !matches!(machine, "LASER-A" | "LASER-B") {
                    return error("MACHINE_NOT_CAPABLE");
                }
                if event_matching(events, "sheet_machine_down", "machine", machine) {
                    return error("MACHINE_UNAVAILABLE");
                }
                let id = self.issued.as_deref().unwrap();
                if event_matching(events, "sheet_supply_shortage", "stock", id) {
                    return error("STOCK_UNAVAILABLE");
                }
                let stock = self.inventory.get_mut(id).unwrap();
                let input = std::mem::take(&mut stock.area_mm2);
                self.ledger.push(json!({"kind":"cut","stock":id,"lot":stock.lot,"parent":stock.parent,"input_mm2":input,
                    "product_mm2":PART_AREA,"scrap_mm2":SCRAP_AREA,"remnant_mm2":input-CUT_AREA,"machine":machine}));
                self.machine = Some(machine.into());
                self.phase = SheetPhase::Cut;
            }
            Action::SheetForm if self.phase == SheetPhase::Cut => self.phase = SheetPhase::Formed,
            Action::SheetInspect if self.phase == SheetPhase::Formed => {
                self.phase = if args["result"] == "pass" {
                    SheetPhase::Inspected
                } else {
                    SheetPhase::Failed
                };
            }
            Action::SheetReturn if self.phase == SheetPhase::Inspected => {
                let cut = &self.ledger[0];
                let parent = self.issued.as_deref().unwrap();
                let original = &self.inventory[parent];
                let area = cut["remnant_mm2"].as_u64().unwrap();
                if area > 0 {
                    self.inventory.insert(
                        "REM-RETURN".into(),
                        Stock {
                            lot: original.lot.clone(),
                            alloy: original.alloy.clone(),
                            area_mm2: area,
                            eligible: true,
                            parent: Some(parent.into()),
                        },
                    );
                }
                self.ledger.push(
                    json!({"kind":"return","stock":if area>0 {Some("REM-RETURN")} else {None},
                    "lot":cut["lot"],"parent":parent,"area_mm2":area}),
                );
                self.phase = SheetPhase::Complete;
            }
            _ => return error("INVALID_STATE"),
        }
        json!({"ok":true,"phase":self.phase,"issued":self.issued,"machine":self.machine,"ledger":self.ledger})
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Wire {
    phase: WirePhase,
    active_terminal: Option<String>,
    history: Vec<Value>,
}

#[derive(Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
enum WirePhase {
    #[default]
    Planned,
    Cut,
    Crimped,
    Held,
    Reworked,
    Passed,
    Released,
}

#[derive(Serialize, Deserialize)]
struct Harness {
    units: BTreeMap<String, Wire>,
    stock: BTreeMap<String, u64>,
    sample_cursor: Option<usize>,
}

impl Default for Harness {
    fn default() -> Self {
        Self {
            units: ["WIRE-01", "WIRE-02"]
                .into_iter()
                .map(|id| (id.into(), Wire::default()))
                .collect(),
            stock: [("TERM-STD", 4), ("TERM-ALT", 4)]
                .into_iter()
                .map(|(id, quantity)| (id.into(), quantity))
                .collect(),
            sample_cursor: None,
        }
    }
}

impl Harness {
    fn apply(&mut self, action: Action, args: &Map<String, Value>, events: &[Event]) -> Value {
        if action == Action::HarnessTrace {
            let subject = args["subject"].as_str().unwrap();
            let units: BTreeMap<_, _> = self
                .units
                .iter()
                .filter(|(serial, wire)| {
                    subject == "WO-HARNESS"
                        || subject == serial.as_str()
                        || wire
                            .history
                            .iter()
                            .any(|item| item["terminal_batch"] == subject)
                })
                .collect();
            return json!({"ok":true,"units":units,"stock":self.stock});
        }
        if action == Action::HarnessSample {
            let cursor = args["event_cursor"].as_u64().unwrap() as usize;
            let Some(event) = events
                .get(cursor - 1)
                .filter(|event| event.kind == "harness_sample_failed")
            else {
                return error("SAMPLE_NOT_DELIVERED");
            };
            if self.sample_cursor == Some(cursor) {
                return error("SAMPLE_ALREADY_APPLIED");
            }
            let mut affected = Vec::new();
            for (serial, wire) in &mut self.units {
                if event.payload["serial"] == serial.as_str()
                    && event.payload["terminal_batch"].as_str() == wire.active_terminal.as_deref()
                {
                    wire.phase = WirePhase::Held;
                    wire.history
                        .push(json!({"operation":"isolate","event_cursor":cursor}));
                    affected.push(serial.clone());
                }
            }
            if affected.is_empty() {
                return error("SAMPLE_SUBJECT_NOT_READY");
            }
            self.sample_cursor = Some(cursor);
            return json!({"ok":true,"affected":affected});
        }
        let serial = args["serial"].as_str().unwrap();
        let Some(wire) = self.units.get_mut(serial) else {
            return error("SERIAL_NOT_FOUND");
        };
        match action {
            Action::HarnessCut if wire.phase == WirePhase::Planned => {
                wire.phase = WirePhase::Cut;
                wire.history.push(json!({"operation":"cut","process_revision":"HARNESS-1","wire_lot":"WIRE-CU-1","planned_terminal":"TERM-STD"}));
            }
            Action::HarnessCrimp | Action::HarnessRework
                if (action == Action::HarnessCrimp && wire.phase == WirePhase::Cut)
                    || (action == Action::HarnessRework && wire.phase == WirePhase::Held) =>
            {
                let batch = args["terminal_batch"].as_str().unwrap();
                let Some(stock) = self.stock.get_mut(batch) else {
                    return error("TERMINAL_NOT_QUALIFIED");
                };
                if *stock == 0 {
                    return error("MATERIAL_SHORTAGE");
                }
                if action == Action::HarnessRework
                    && events.iter().any(|event| {
                        event.kind == "harness_sample_failed"
                            && event.payload["terminal_batch"] == batch
                    })
                {
                    return error("TERMINAL_QUALITY_HOLD");
                }
                *stock -= 1;
                let rework = action == Action::HarnessRework;
                wire.history.push(json!({"operation":if rework {"rework"} else {"crimp"},"terminal_batch":batch,
                    "previous_terminal":wire.active_terminal,"qualification":if batch=="TERM-ALT" {Some("QA-TERMINAL-12")} else {None},
                    "process_revision":if rework {"HARNESS-2"} else {"HARNESS-1"},"reason":args.get("reason")}));
                wire.active_terminal = Some(batch.into());
                wire.phase = if rework {
                    WirePhase::Reworked
                } else {
                    WirePhase::Crimped
                };
            }
            Action::HarnessTest
                if matches!(wire.phase, WirePhase::Crimped | WirePhase::Reworked) =>
            {
                if event_matching(events, "harness_sample_failed", "serial", serial)
                    && wire.active_terminal.as_deref() == Some("TERM-ALT")
                {
                    return error("QUALITY_HOLD");
                }
                let passed = args["result"] == "pass";
                wire.history.push(json!({"operation":"continuity_test","result":args["result"],"terminal_batch":wire.active_terminal}));
                wire.phase = if passed {
                    WirePhase::Passed
                } else {
                    WirePhase::Held
                };
            }
            Action::HarnessRelease if wire.phase == WirePhase::Passed => {
                if event_matching(events, "harness_sample_failed", "serial", serial)
                    && wire.active_terminal.as_deref() == Some("TERM-ALT")
                {
                    return error("QUALITY_HOLD");
                }
                wire.phase = WirePhase::Released;
            }
            Action::HarnessRelease => return error("TEST_PASS_REQUIRED"),
            _ => return error("INVALID_STATE"),
        }
        json!({"ok":true,"serial":serial,"phase":wire.phase,"history":wire.history})
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "industry", content = "state", rename_all = "snake_case")]
enum State {
    Sheet(Sheet),
    Nesting(nesting::Nesting),
    Harness(Harness),
}

pub fn invoke(
    tx: &Transaction<'_>,
    run: &str,
    setup: Setup,
    orders: &[String],
    action: Action,
    args: &Map<String, Value>,
    cursor: i64,
    events: &[Event],
) -> anyhow::Result<(Value, bool)> {
    let key = args.get("idempotency_key").and_then(Value::as_str);
    let request = json!({"action":action,"arguments":args}).to_string();
    if let Some(key) = key {
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
    }
    let persisted: Option<String> = tx
        .query_row(
            "SELECT state FROM fabrication_runs WHERE run=?1",
            [run],
            |row| row.get(0),
        )
        .optional()?;
    let mut state = match persisted {
        Some(value) => serde_json::from_str(&value)?,
        None => match setup {
            Setup::SheetMetal => State::Sheet(Sheet::default()),
            Setup::SheetNesting => State::Nesting(nesting::Nesting::default()),
            Setup::SheetDimensionalNesting
            | Setup::SheetRemnantNesting
            | Setup::SheetLayoutNesting => State::Nesting(nesting::Nesting::dimensional()),
            Setup::WireHarness => State::Harness(Harness::default()),
        },
    };
    let mut response = match &mut state {
        State::Sheet(sheet) => sheet.apply(action, args, events),
        State::Nesting(nesting) => nesting.apply(action, args, events, orders, setup),
        State::Harness(harness) => harness.apply(action, args, events),
    };
    if let Some(key) = key.filter(|_| response["ok"] == true) {
        response["cursor"] = (cursor + 1).into();
        tx.execute("INSERT INTO fabrication_runs VALUES (?1,?2) ON CONFLICT(run) DO UPDATE SET state=excluded.state",
            params![run,serde_json::to_string(&state)?])?;
        tx.execute(
            "UPDATE scenario_runs SET cursor=cursor+1 WHERE id=?1",
            [run],
        )?;
        tx.execute(
            "INSERT INTO scenario_requests VALUES (?1,?2,?3,?4)",
            params![run, key, request, response.to_string()],
        )?;
        return Ok((response, true));
    }
    Ok((response, false))
}
