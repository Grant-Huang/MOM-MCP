use std::collections::{BTreeMap, BTreeSet};

use mom_des::Event;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::catalog::{Action, Spec, object, text};

pub const TOOLS: &[(Action, &str, &str, &str)] = &[
    (
        Action::SmtRelease,
        "smt_release",
        "下达 SMT 工单",
        "production",
    ),
    (
        Action::SmtLoad,
        "smt_load",
        "料卷换料与飞达上料",
        "inventory",
    ),
    (
        Action::SmtSpliceReel,
        "smt_splice_reel",
        "在飞达上登记相同物料版本的接料",
        "production",
    ),
    (Action::SmtPlace, "smt_place", "贴装拼板", "production"),
    (
        Action::SmtReviewAoi,
        "smt_review_aoi",
        "复判拼板 AOI 失败并逐板登记处置",
        "quality",
    ),
    (Action::SmtAoi, "smt_aoi", "拼板 AOI 检测", "quality"),
    (
        Action::SmtSplit,
        "smt_split",
        "分板并登记单板序列号",
        "production",
    ),
    (
        Action::SmtFinish,
        "smt_finish",
        "单板完工放行",
        "production",
    ),
    (
        Action::SmtStatus,
        "smt_status",
        "查询拼板、单板和上料历史",
        "production",
    ),
    (
        Action::SmtTrace,
        "smt_trace",
        "料卷、拼板、单板双向追溯",
        "inventory",
    ),
    (
        Action::SmtGetRecallImpact,
        "smt_get_recall_impact",
        "预检召回涉及的工单、拼板和单板",
        "quality",
    ),
    (
        Action::SmtIsolate,
        "smt_isolate",
        "按召回通知隔离受影响板卡",
        "quality",
    ),
    (
        Action::SmtRework,
        "smt_rework",
        "更换单板受影响元件",
        "quality",
    ),
    (Action::SmtRetest, "smt_retest", "返工单板复检", "quality"),
    (Action::SmtScrap, "smt_scrap", "报废单板", "quality"),
];

pub fn spec(action: Action) -> Spec {
    let &(_, name, description, domain) = TOOLS.iter().find(|entry| entry.0 == action).unwrap();
    let mut required = match action {
        Action::SmtRelease | Action::SmtStatus => vec!["work_order"],
        Action::SmtLoad => vec!["slot", "reel"],
        Action::SmtSpliceReel => vec!["slot", "reel", "expected_cursor"],
        Action::SmtPlace => vec!["panel", "slot"],
        Action::SmtAoi => vec!["panel", "result"],
        Action::SmtReviewAoi => vec![
            "panel",
            "decisions",
            "responsible",
            "reason",
            "evidence",
            "expected_cursor",
        ],
        Action::SmtSplit => vec!["panel"],
        Action::SmtFinish => vec!["serial"],
        Action::SmtTrace => vec!["subject"],
        Action::SmtIsolate | Action::SmtGetRecallImpact => vec!["event_cursor"],
        Action::SmtRework => vec!["serial", "replacement_reel", "reason"],
        Action::SmtRetest => vec!["serial", "result"],
        Action::SmtScrap => vec!["serial", "reason"],
        _ => unreachable!(),
    };
    let read = matches!(
        action,
        Action::SmtStatus | Action::SmtTrace | Action::SmtGetRecallImpact
    );
    if !read {
        required.push("idempotency_key");
    }
    let fields = required
        .iter()
        .map(|&field| {
            (
                field.into(),
                match field {
                    "decisions" => json!({"type":"object","minProperties":1,"maxProperties":64,
                        "additionalProperties":{"enum":["pass","hold"]}}),
                    "result" => json!({"type":"string","enum":["pass","fail"]}),
                    "expected_cursor" => json!({"type":"integer","minimum":0,"maximum":i64::MAX}),
                    "event_cursor" => json!({"type":"integer","minimum":1,"maximum":10000}),
                    _ => text(),
                },
            )
        })
        .collect::<Map<_, _>>();
    let output = if action == Action::SmtReviewAoi {
        let cursor = json!({"type":"integer","minimum":1});
        let review = object(
            &["decisions", "responsible", "reason", "evidence", "cursor"],
            json!({"decisions":fields["decisions"],"responsible":text(),"reason":text(),"evidence":text(),"cursor":cursor}),
        );
        json!({"oneOf":[
            object(&["ok","panel","aoi_review","cursor"],json!({"ok":{"const":true},"panel":text(),"aoi_review":review,"cursor":cursor})),
            object(&["ok","code"],json!({"ok":{"const":false},"code":text()}))
        ]})
    } else if action == Action::SmtGetRecallImpact {
        let target = object(
            &[
                "work_order",
                "panel",
                "serial",
                "actual_reel",
                "split",
                "previously_finished",
            ],
            json!({"work_order":text(),"panel":text(),"serial":text(),"actual_reel":text(),
                "split":{"type":"boolean"},"previously_finished":{"type":"boolean"}}),
        );
        json!({"oneOf":[
            object(&["ok","reel","event_cursor","targets"],json!({"ok":{"const":true},"reel":text(),
                "event_cursor":fields["event_cursor"],"targets":{"type":"array","items":target}})),
            object(&["ok","code"],json!({"ok":{"const":false},"code":text()}))
        ]})
    } else {
        json!({"type":"object","required":["ok"],"properties":{"ok":{"type":"boolean"},"code":{"type":"string"}}})
    };
    Spec {
        name,
        description,
        domain,
        effect: if read { "read" } else { "write" },
        input: object(&required, Value::Object(fields)),
        output,
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: if read { "none" } else { "update_smt_state" },
        errors: &[
            "INVALID_STATE",
            "REEL_RECALLED",
            "RECALL_NOT_DELIVERED",
            "RECALL_ALREADY_ISOLATED",
            "QUALITY_HOLD",
            "MATERIAL_HELD",
            "REEL_NOT_FOUND",
            "FEEDER_EMPTY",
            "SPLICE_MATERIAL_MISMATCH",
            "AOI_REVIEW_REQUIRED",
            "AOI_REVIEW_NOT_ELIGIBLE",
            "INVALID_ARGUMENT",
            "VERSION_CONFLICT",
            "RETEST_REQUIRED",
            "BOARD_SCRAPPED",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shared_material: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    split_reels: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) spliced_placement: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) aoi_review: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) recall_impact: bool,
    reels: BTreeMap<String, usize>,
    panels: BTreeMap<String, PanelDefinition>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PanelDefinition {
    order: String,
    serials: Vec<String>,
}

impl Setup {
    pub fn validate(
        &self,
        orders: &[String],
        model: &str,
        parameters: &Map<String, Value>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            model == mom_model_smt::ID,
            "smt setup requires smt_line model"
        );
        anyhow::ensure!(
            parameters
                .get("recall_reel")
                .and_then(Value::as_str)
                .is_some_and(|id| self.reels.contains_key(id)),
            "model_parameters.recall_reel references unknown reel"
        );
        anyhow::ensure!(
            parameters
                .get("aoi_panel")
                .and_then(Value::as_str)
                .is_some_and(|id| self.panels.contains_key(id)),
            "model_parameters.aoi_panel references unknown panel"
        );
        anyhow::ensure!(
            !self.split_reels || self.shared_material,
            "split reels require the shared material ledger"
        );
        anyhow::ensure!(
            !self.spliced_placement || self.split_reels,
            "spliced placement requires split reel support"
        );
        let mut serials = BTreeSet::new();
        for (panel, spec) in &self.panels {
            anyhow::ensure!(
                orders.contains(&spec.order),
                "smt.panels.{panel}.order references unknown work order"
            );
            anyhow::ensure!(
                !spec.serials.is_empty() && spec.serials.len() <= 64,
                "invalid smt.panels.{panel}.serials"
            );
            for serial in &spec.serials {
                anyhow::ensure!(
                    !serial.is_empty() && serials.insert(serial),
                    "invalid or duplicate SMT serial {serial}"
                );
            }
        }
        Ok(())
    }
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    released: BTreeSet<String>,
    slots: BTreeMap<String, usize>,
    load_history: Vec<FeederLoad>,
    panels: BTreeMap<String, Panel>,
    boards: BTreeMap<String, Board>,
    isolated: BTreeSet<usize>,
    remaining: BTreeMap<String, usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    material_reports: Vec<crate::mes::Report>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    crossed_splices: BTreeSet<usize>,
}

#[derive(Serialize, Deserialize)]
struct FeederLoad {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_load_id: Option<usize>,
    id: usize,
    reel: String,
    slot: String,
    machine: String,
}

#[derive(Serialize, Deserialize)]
struct Panel {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    placement_sources: BTreeMap<String, PlacementSource>,
    reel: String,
    load_id: usize,
    slot: String,
    aoi: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    aoi_review: Option<AoiReview>,
    split: bool,
}

#[derive(Serialize, Deserialize)]
struct AoiReview {
    decisions: BTreeMap<String, String>,
    responsible: String,
    reason: String,
    evidence: String,
    cursor: i64,
}

#[derive(Serialize, Deserialize)]
struct PlacementSource {
    reel: String,
    load_id: usize,
}

impl Panel {
    fn reel_for(&self, serial: &str) -> &str {
        self.placement_sources
            .get(serial)
            .map_or(self.reel.as_str(), |source| &source.reel)
    }
}

#[derive(Serialize, Deserialize)]
struct Board {
    panel: String,
    quality: Quality,
    finished: bool,
    history: Vec<Value>,
    #[serde(default)]
    replacements: Vec<String>,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Quality {
    Clear,
    Held,
    AwaitingRetest,
    Scrapped,
}

fn load(tx: &Transaction<'_>, run: &str, setup: &Setup) -> anyhow::Result<State> {
    let stored: Option<String> = tx
        .query_row("SELECT state FROM smt_runs WHERE run=?1", [run], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(stored.map_or_else(
        || {
            Ok(State {
                remaining: if setup.shared_material {
                    BTreeMap::new()
                } else {
                    setup.reels.clone()
                },
                ..State::default()
            })
        },
        |state| serde_json::from_str(&state),
    )?)
}

fn failure(code: &str) -> Value {
    json!({"ok":false,"code":code})
}

fn recall(event: &Event) -> Option<&str> {
    (event.kind == "supplier_recall")
        .then(|| event.payload["reel"].as_str())
        .flatten()
}

fn current_reel<'a>(board: &'a Board, panel: &'a Panel, serial: &str) -> &'a str {
    board
        .replacements
        .last()
        .map(String::as_str)
        .unwrap_or_else(|| panel.reel_for(serial))
}

fn affected_reel(parents: &BTreeMap<String, Option<String>>, reel: &str, recalled: &str) -> bool {
    reel == recalled || parents.get(reel).and_then(Option::as_deref) == Some(recalled)
}

fn reel_parents(
    tx: &Transaction<'_>,
    run: &str,
) -> rusqlite::Result<BTreeMap<String, Option<String>>> {
    let mut query = tx.prepare("SELECT lot,parent_lot FROM stock_available_lots WHERE run=?1 AND posted_cursor IS NOT NULL AND unit='each'")?;
    query
        .query_map([run], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}

#[derive(Serialize)]
struct RecallTarget<'a> {
    work_order: &'a str,
    panel: &'a str,
    serial: &'a str,
    actual_reel: &'a str,
    split: bool,
    previously_finished: bool,
}

fn recall_targets<'a>(
    state: &'a State,
    setup: &'a Setup,
    parents: &BTreeMap<String, Option<String>>,
    reel: &str,
) -> Vec<RecallTarget<'a>> {
    let mut targets = Vec::new();
    for (id, panel) in &state.panels {
        let definition = &setup.panels[id];
        for serial in &definition.serials {
            let board = state.boards.get(serial);
            let actual = board.map_or_else(
                || panel.reel_for(serial),
                |board| current_reel(board, panel, serial),
            );
            if board.is_some_and(|board| board.quality == Quality::Scrapped)
                || !affected_reel(parents, actual, reel)
            {
                continue;
            }
            targets.push(RecallTarget {
                work_order: &definition.order,
                panel: id,
                serial,
                actual_reel: actual,
                split: board.is_some(),
                previously_finished: board.is_some_and(|board| board.finished),
            });
        }
    }
    targets
}

pub fn work_order_status(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    order: &str,
) -> anyhow::Result<&'static str> {
    let state = load(tx, run, setup)?;
    if !state.released.contains(order) {
        return Ok("planned");
    }
    let complete = setup
        .panels
        .values()
        .filter(|panel| panel.order == order)
        .all(|panel| {
            panel.serials.iter().all(|serial| {
                state
                    .boards
                    .get(serial)
                    .is_some_and(|board| board.finished || board.quality == Quality::Scrapped)
            })
        });
    Ok(if complete { "complete" } else { "released" })
}

pub(crate) const PLACEMENT: &str = "placement";
pub(crate) const WORK_CENTER: &str = "SMT-1";

pub(crate) fn valid_frozen_placement(snapshot: &Value) -> bool {
    let placement = &snapshot["route"]["operations"][PLACEMENT];
    placement["work_center"] == WORK_CENTER && placement["predecessors"] == json!([])
}

pub(crate) fn planned_units(setup: &Setup, order: &str) -> i64 {
    setup
        .panels
        .values()
        .filter(|panel| panel.order == order)
        .map(|panel| panel.serials.len() as i64)
        .sum()
}

pub(crate) fn can_issue(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    order: &str,
) -> anyhow::Result<bool> {
    let state = load(tx, run, setup)?;
    Ok(state.released.contains(order)
        && setup
            .panels
            .iter()
            .any(|(id, panel)| panel.order == order && !state.panels.contains_key(id)))
}

pub(crate) fn genealogy(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    action: Action,
    args: &Map<String, Value>,
) -> anyhow::Result<Value> {
    crate::genealogy::read(
        tx,
        run,
        action,
        &load(tx, run, setup)?.material_reports,
        args,
        if setup.split_reels {
            crate::genealogy::Detail::MaterialSplits
        } else {
            crate::genealogy::Detail::Standard
        },
    )
}

pub(crate) fn placement_spec(mut spec: Spec, spliced: bool) -> Spec {
    spec.input["properties"]["reservation_id"] = crate::inventory::id_schema();
    spec.input["properties"]["expected_cursor"] =
        json!({"type":"integer","minimum":0,"maximum":i64::MAX});
    spec.input["required"]
        .as_array_mut()
        .unwrap()
        .extend([json!("reservation_id"), json!("expected_cursor")]);
    if spliced {
        spec.input["required"]
            .as_array_mut()
            .unwrap()
            .retain(|field| field != "reservation_id");
        spec.input["properties"]["reservation_ids"] = json!({"type":"array","minItems":1,"maxItems":64,
            "items":crate::inventory::id_schema(),"description":"按拼板序列号的登记顺序填写每块板的发料编号"});
    }
    spec.errors = &[
        "INVALID_ARGUMENT",
        "INVALID_STATE",
        "WORK_ORDER_NOT_FOUND",
        "PANEL_NOT_FOUND",
        "FEEDER_EMPTY",
        "VERSION_CONFLICT",
        "IDEMPOTENCY_CONFLICT",
        "REEL_RECALLED",
        "QUALITY_HOLD",
        "QUALITY_SCRAPPED",
        "MATERIAL_HELD",
        "MATERIAL_ISSUE_NOT_FOUND",
        "MATERIAL_ISSUE_QUANTITY_EXCEEDED",
        "FEEDER_LOT_MISMATCH",
        "SPLICE_SEQUENCE_INVALID",
        "MATERIAL_UNIT_MISMATCH",
    ];
    spec
}

pub fn invoke(
    tx: &Transaction<'_>,
    run: &str,
    setup: &Setup,
    action: Action,
    args: &Map<String, Value>,
    cursor: i64,
    events: &[Event],
) -> anyhow::Result<(Value, bool)> {
    let mut state = load(tx, run, setup)?;
    if action == Action::SmtStatus {
        let order = args["work_order"].as_str().unwrap();
        if !setup.panels.values().any(|panel| panel.order == order) {
            return Ok((failure("WORK_ORDER_NOT_FOUND"), false));
        }
        let panels: BTreeMap<_, _> = state
            .panels
            .iter()
            .filter(|(id, _)| setup.panels[*id].order == order)
            .collect();
        let boards: BTreeMap<_, _> = state
            .boards
            .iter()
            .filter(|(_, board)| setup.panels[&board.panel].order == order)
            .collect();
        return Ok((
            json!({"ok":true,"work_order":order,"released":state.released.contains(order),"panels":panels,"boards":boards,
            "load_history":state.load_history,"remaining":state.remaining,"cursor":cursor}),
            false,
        ));
    }
    if action == Action::SmtTrace {
        let subject = args["subject"].as_str().unwrap();
        let links: Vec<_>=state.boards.iter().filter_map(|(serial,board)| {
            let panel=&state.panels[&board.panel];
            (subject==serial || subject==board.panel || subject==panel.reel_for(serial) || board.replacements.iter().any(|id| id==subject)).then(||
                json!({"serial":serial,"panel":board.panel,"work_order":setup.panels[&board.panel].order,
                    "reel":panel.reel_for(serial),"slot":panel.slot,"machine":"SMT-1","load_id":panel.placement_sources.get(serial).map_or(panel.load_id,|source|source.load_id),"aoi":panel.aoi,
                    "replacements":board.replacements,"quality":board.quality,"finished":board.finished,"history":board.history}))
        }).collect();
        let panels: BTreeMap<_, _> = state
            .panels
            .iter()
            .filter(|(id, panel)| {
                subject == id.as_str()
                    || subject == panel.reel
                    || panel
                        .placement_sources
                        .values()
                        .any(|source| source.reel == subject)
            })
            .collect();
        return Ok((json!({"ok":true,"links":links,"panels":panels}), false));
    }
    if action == Action::SmtGetRecallImpact {
        let event_cursor = args["event_cursor"].as_u64().unwrap() as usize;
        let Some(reel) = events.get(event_cursor - 1).and_then(recall) else {
            return Ok((failure("RECALL_NOT_DELIVERED"), false));
        };
        let parents = if setup.split_reels {
            reel_parents(tx, run)?
        } else {
            BTreeMap::new()
        };
        let targets = recall_targets(&state, setup, &parents, reel);
        return Ok((
            json!({"ok":true,"reel":reel,"event_cursor":event_cursor,"targets":targets}),
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
                failure("IDEMPOTENCY_CONFLICT")
            },
            false,
        ));
    }
    if matches!(
        action,
        Action::ReleaseWorkOrder | Action::SmtSpliceReel | Action::SmtReviewAoi
    ) || (setup.shared_material && action == Action::SmtPlace)
    {
        if args["expected_cursor"].as_i64() != Some(cursor) {
            return Ok((failure("VERSION_CONFLICT"), false));
        }
    }
    if setup.shared_material {
        let affected = match action {
            Action::SmtPlace | Action::SmtAoi | Action::SmtReviewAoi | Action::SmtSplit => {
                args.get("panel").and_then(Value::as_str)
            }
            Action::SmtFinish => args
                .get("serial")
                .and_then(Value::as_str)
                .and_then(|serial| state.boards.get(serial))
                .map(|board| board.panel.as_str()),
            _ => None,
        };
        if let Some(panel) = affected.and_then(|id| setup.panels.get(id)) {
            if let Some(code) = crate::quality::blocking(tx, run, &panel.order)? {
                return Ok((failure(&code), false));
            }
            if action != Action::SmtPlace {
                let held = if action == Action::SmtFinish
                    && let Some(serial) = args["serial"].as_str()
                    && let Some(source) = affected
                        .and_then(|id| state.panels.get(id))
                        .and_then(|panel| panel.placement_sources.get(serial))
                {
                    tx.query_row("SELECT EXISTS(SELECT 1 FROM stock_effective_lot_holds WHERE run=?1 AND batch=?2)",
                        params![run,source.reel], |row| row.get(0))?
                } else if let Some(report) = state.material_reports.iter().find(|report| {
                    report.order == panel.order
                        && report.serial_numbers == panel.serials
                        && !report.reversed
                }) {
                    crate::wip::material_held(tx, run, report)?
                } else {
                    false
                };
                if held {
                    return Ok((failure("MATERIAL_HELD"), false));
                }
            }
        }
    }
    let mut reel_parents = BTreeMap::new();
    if setup.split_reels {
        reel_parents = self::reel_parents(tx, run)?;
        if matches!(action, Action::SmtLoad | Action::SmtSpliceReel) {
            let reel = args["reel"].as_str().unwrap();
            if !reel_parents.contains_key(reel) {
                return Ok((failure("REEL_NOT_FOUND"), false));
            }
            let held: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM stock_effective_lot_holds WHERE run=?1 AND batch=?2)",
                params![run, reel],
                |row| row.get(0),
            )?;
            if held {
                return Ok((failure("MATERIAL_HELD"), false));
            }
        }
    }
    if action == Action::SmtSpliceReel {
        let slot = args["slot"].as_str().unwrap();
        let reel = args["reel"].as_str().unwrap();
        let Some(load_id) = state.slots.get(slot) else {
            return Ok((failure("FEEDER_EMPTY"), false));
        };
        let old = &state.load_history[*load_id - 1];
        let same_material: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM stock_available_lots a JOIN stock_available_lots b
             ON b.run=a.run AND b.item=a.item AND b.revision=a.revision
             WHERE a.run=?1 AND a.lot=?2 AND b.lot=?3)",
            params![run, old.reel, reel],
            |row| row.get(0),
        )?;
        if !same_material
            || old.reel == reel
            || (old.previous_load_id.is_some() && !state.crossed_splices.contains(load_id))
        {
            return Ok((failure("SPLICE_MATERIAL_MISMATCH"), false));
        }
    }
    let mut response = apply(&mut state, setup, action, args, events, &reel_parents);
    if response["ok"] != true {
        return Ok((response, false));
    }
    if setup.shared_material && action == Action::SmtPlace {
        let panel = args["panel"].as_str().unwrap();
        let definition = &setup.panels[panel];
        let amount = definition.serials.len();
        let reservation = args.get("reservation_id").and_then(Value::as_str);
        let ordered = args.get("reservation_ids").and_then(Value::as_array);
        if reservation.is_some() == ordered.is_some()
            || ordered.is_some_and(|ids| ids.len() != amount)
        {
            return Ok((failure("INVALID_ARGUMENT"), false));
        }
        let ordered_ids: Vec<_> = ordered.map_or_else(
            || vec![reservation.unwrap(); amount],
            |ids| ids.iter().map(|id| id.as_str().unwrap()).collect(),
        );
        let mut serials_by_reservation: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (reservation, serial) in ordered_ids.iter().zip(&definition.serials) {
            serials_by_reservation
                .entry((*reservation).to_owned())
                .or_default()
                .push(serial.clone());
        }
        let quantities: BTreeMap<_, _> = serials_by_reservation
            .iter()
            .map(|(reservation, serials)| (reservation, serials.len().to_string()))
            .collect();
        let query = json!({"operation":PLACEMENT,"material_inputs":quantities});
        let inputs = match crate::mes::report_material_inputs(
            tx,
            run,
            &state.material_reports,
            &definition.order,
            query.as_object().unwrap(),
        )? {
            Ok(inputs) => inputs,
            Err(code) => return Ok((failure(code), false)),
        };
        if inputs.iter().any(|input| input.unit != "each") {
            return Ok((failure("MATERIAL_UNIT_MISMATCH"), false));
        }
        let panel_state = state.panels.get_mut(panel).unwrap();
        let active = &state.load_history[panel_state.load_id - 1];
        let previous = active
            .previous_load_id
            .filter(|_| !state.crossed_splices.contains(&active.id))
            .map(|id| &state.load_history[id - 1]);
        let mut crossed = false;
        for (reservation, serial) in ordered_ids.iter().zip(&definition.serials) {
            let input = inputs
                .iter()
                .find(|input| input.reservation_id == *reservation)
                .unwrap();
            let source = if input.lot_id == active.reel {
                crossed = true;
                active
            } else if let Some(previous) = previous.filter(|load| load.reel == input.lot_id) {
                if crossed {
                    return Ok((failure("SPLICE_SEQUENCE_INVALID"), false));
                }
                previous
            } else {
                return Ok((failure("FEEDER_LOT_MISMATCH"), false));
            };
            if events.iter().filter_map(recall).any(|root| {
                source.reel == root
                    || reel_parents.get(&source.reel).and_then(Option::as_deref) == Some(root)
            }) {
                return Ok((failure("REEL_RECALLED"), false));
            }
            if setup.spliced_placement {
                panel_state.placement_sources.insert(
                    serial.clone(),
                    PlacementSource {
                        reel: source.reel.clone(),
                        load_id: source.id,
                    },
                );
            }
        }
        if crossed && active.previous_load_id.is_some() {
            state.crossed_splices.insert(active.id);
        }
        let report = crate::mes::Report {
            id: format!("report-{}", cursor + 1),
            order: definition.order.clone(),
            step: 0,
            batch: (inputs.len() == 1).then(|| inputs[0].lot_id.clone()),
            quantity: amount,
            reversed: false,
            serial_numbers: definition.serials.clone(),
            components: BTreeMap::new(),
            material_inputs: inputs,
            material_input_serials: if setup.spliced_placement {
                serials_by_reservation
            } else {
                BTreeMap::new()
            },
            wip_move: None,
            wip_movement_reversals: Vec::new(),
            wip_partitions: Vec::new(),
            wip_inputs: Vec::new(),
            wip_sources: Vec::new(),
            consolidated_into: None,
            coating: None,
            tool_setup: None,
        };
        response["report_id"] = json!(report.id);
        state.material_reports.push(report);
    }
    response["cursor"] = (cursor + 1).into();
    tx.execute(
        "INSERT INTO smt_runs VALUES (?1,?2) ON CONFLICT(run) DO UPDATE SET state=excluded.state",
        params![run, serde_json::to_string(&state)?],
    )?;
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
    args: &Map<String, Value>,
    events: &[Event],
    reel_parents: &BTreeMap<String, Option<String>>,
) -> Value {
    let string = |field: &str| args[field].as_str().unwrap();
    let matches_recall = |reel: &str, recalled: &str| affected_reel(reel_parents, reel, recalled);
    let recalled = |reel: &str| {
        events
            .iter()
            .filter_map(recall)
            .any(|root| matches_recall(reel, root))
    };
    match action {
        Action::SmtRelease | Action::ReleaseWorkOrder => {
            let order = string("work_order");
            if !setup.panels.values().any(|panel| panel.order == order) {
                return failure("WORK_ORDER_NOT_FOUND");
            }
            if !state.released.insert(order.into()) {
                return failure("INVALID_STATE");
            }
        }
        Action::SmtLoad | Action::SmtSpliceReel => {
            let reel = string("reel");
            let slot = string("slot");
            if !matches!(slot, "F1" | "F2") {
                return failure("SLOT_NOT_FOUND");
            }
            if !setup.split_reels && !setup.reels.contains_key(reel) {
                return failure("REEL_NOT_FOUND");
            }
            if recalled(reel) {
                return failure("REEL_RECALLED");
            }
            let id = state.load_history.len() + 1;
            let previous_load_id = (action == Action::SmtSpliceReel).then(|| state.slots[slot]);
            state.slots.insert(slot.into(), id);
            state.load_history.push(FeederLoad {
                id,
                previous_load_id,
                reel: reel.into(),
                slot: slot.into(),
                machine: "SMT-1".into(),
            });
            return json!({"ok":true,"load_id":id});
        }
        Action::SmtPlace => {
            let id = string("panel");
            let Some(spec) = setup.panels.get(id) else {
                return failure("PANEL_NOT_FOUND");
            };
            if !state.released.contains(&spec.order) || state.panels.contains_key(id) {
                return failure("INVALID_STATE");
            }
            let slot = string("slot");
            let Some(&load_id) = state.slots.get(slot) else {
                return failure("FEEDER_EMPTY");
            };
            let reel = &state.load_history[load_id - 1].reel;
            if recalled(reel) {
                return failure("REEL_RECALLED");
            }
            if !setup.shared_material {
                let available = state.remaining.get_mut(reel).unwrap();
                if *available < spec.serials.len() {
                    return failure("MATERIAL_SHORTAGE");
                }
                *available -= spec.serials.len();
            }
            state.panels.insert(
                id.into(),
                Panel {
                    placement_sources: BTreeMap::new(),
                    reel: reel.clone(),
                    load_id,
                    slot: slot.into(),
                    aoi: None,
                    aoi_review: None,
                    split: false,
                },
            );
        }
        Action::SmtReviewAoi => {
            let id = string("panel");
            let Some(panel) = state.panels.get_mut(id) else {
                return failure("AOI_REVIEW_NOT_ELIGIBLE");
            };
            if panel.aoi.as_deref() != Some("fail") || panel.split || panel.aoi_review.is_some() {
                return failure("AOI_REVIEW_NOT_ELIGIBLE");
            }
            let serials = &setup.panels[id].serials;
            let decisions = args["decisions"].as_object().unwrap();
            if decisions.len() != serials.len()
                || serials.iter().any(|serial| {
                    !matches!(
                        decisions.get(serial).and_then(Value::as_str),
                        Some("pass" | "hold")
                    )
                })
                || ["responsible", "reason", "evidence"]
                    .iter()
                    .any(|field| string(field).trim().is_empty())
            {
                return failure("INVALID_ARGUMENT");
            }
            panel.aoi_review = Some(AoiReview {
                decisions: decisions
                    .iter()
                    .map(|(serial, decision)| (serial.clone(), decision.as_str().unwrap().into()))
                    .collect(),
                responsible: string("responsible").into(),
                reason: string("reason").into(),
                evidence: string("evidence").into(),
                cursor: args["expected_cursor"].as_i64().unwrap() + 1,
            });
            return json!({"ok":true,"panel":id,"aoi_review":panel.aoi_review});
        }
        Action::SmtAoi | Action::SmtSplit => {
            let id = string("panel");
            let Some(panel) = state.panels.get_mut(id) else {
                return failure("INVALID_STATE");
            };
            if action == Action::SmtAoi {
                if panel.aoi.is_some() {
                    return failure("INVALID_STATE");
                }
                let drift = events
                    .iter()
                    .any(|event| event.kind == "aoi_drift" && event.payload["panel"] == id);
                if drift && string("result") == "pass" {
                    return failure("AOI_DRIFT_DETECTED");
                }
                panel.aoi = Some(string("result").into());
            } else {
                let Some(aoi) = panel.aoi.as_deref() else {
                    return failure("AOI_REQUIRED");
                };
                if panel.split {
                    return failure("INVALID_STATE");
                }
                if setup.aoi_review && aoi == "fail" && panel.aoi_review.is_none() {
                    return failure("AOI_REVIEW_REQUIRED");
                }
                panel.split = true;
                let serials = &setup.panels[id].serials;
                for serial in serials {
                    let reviewed = panel
                        .aoi_review
                        .as_ref()
                        .map(|review| &review.decisions[serial]);
                    let quality = if recalled(panel.reel_for(serial))
                        || (aoi == "fail" && reviewed.is_none_or(|result| result == "hold"))
                    {
                        Quality::Held
                    } else {
                        Quality::Clear
                    };
                    let mut history = vec![json!({"action":"split","aoi":aoi})];
                    if let Some(review) = &panel.aoi_review {
                        history.push(json!({"action":"aoi_review","decision":review.decisions[serial],
                            "responsible":review.responsible,"reason":review.reason,"evidence":review.evidence,"cursor":review.cursor}));
                    }
                    state.boards.insert(
                        serial.clone(),
                        Board {
                            panel: id.into(),
                            quality,
                            finished: false,
                            history,
                            replacements: vec![],
                        },
                    );
                }
                return json!({"ok":true,"panel":id,"serial_numbers":serials});
            }
        }
        Action::SmtIsolate => {
            let cursor = args["event_cursor"].as_u64().unwrap() as usize;
            let Some(reel) = events.get(cursor - 1).and_then(recall) else {
                return failure("RECALL_NOT_DELIVERED");
            };
            if !state.isolated.insert(cursor) {
                return failure("RECALL_ALREADY_ISOLATED");
            }
            if setup.recall_impact {
                let targets = recall_targets(state, setup, reel_parents, reel);
                let affected: Vec<_> = targets
                    .iter()
                    .filter(|target| target.split)
                    .map(|target| target.serial.to_owned())
                    .collect();
                let pending: Vec<_> = targets
                    .iter()
                    .filter(|target| !target.split)
                    .map(|target| target.serial)
                    .collect();
                let panels: BTreeSet<_> = targets.iter().map(|target| target.panel).collect();
                let result = json!({"ok":true,"affected":affected,"affected_panels":panels,
                    "pending_serials":pending,"reel":reel,"event_cursor":cursor});
                for serial in affected {
                    let board = state.boards.get_mut(&serial).unwrap();
                    board.quality = Quality::Held;
                    board.finished = false;
                    board
                        .history
                        .push(json!({"action":"isolate","event_cursor":cursor}));
                }
                return result;
            }
            let mut affected = Vec::new();
            for (serial, board) in &mut state.boards {
                if matches_recall(
                    current_reel(board, &state.panels[&board.panel], serial),
                    reel,
                ) && board.quality != Quality::Scrapped
                {
                    board.quality = Quality::Held;
                    board.finished = false;
                    board
                        .history
                        .push(json!({"action":"isolate","event_cursor":cursor}));
                    affected.push(serial);
                }
            }
            let panels: Vec<_> = state
                .panels
                .iter()
                .filter(|(_, panel)| {
                    if panel.placement_sources.is_empty() {
                        matches_recall(&panel.reel, reel)
                    } else {
                        panel
                            .placement_sources
                            .values()
                            .any(|source| matches_recall(&source.reel, reel))
                    }
                })
                .map(|(id, _)| id)
                .collect();
            return json!({"ok":true,"affected":affected,"affected_panels":panels,"reel":reel,"event_cursor":cursor});
        }
        _ => {
            let serial = string("serial");
            let Some(board) = state.boards.get_mut(serial) else {
                return failure("BOARD_NOT_FOUND");
            };
            if board.quality == Quality::Scrapped {
                return failure("BOARD_SCRAPPED");
            }
            let reel = current_reel(board, &state.panels[&board.panel], serial);
            match action {
                Action::SmtFinish => {
                    if board.quality == Quality::AwaitingRetest {
                        return failure("RETEST_REQUIRED");
                    }
                    if board.quality == Quality::Held || recalled(reel) {
                        return failure("QUALITY_HOLD");
                    }
                    if board.finished {
                        return failure("INVALID_STATE");
                    }
                    board.finished = true;
                    board.history.push(json!({"action":"finish"}));
                }
                Action::SmtRework if board.quality == Quality::Held => {
                    let replacement = string("replacement_reel");
                    if recalled(replacement) {
                        return failure("REEL_RECALLED");
                    }
                    let Some(available) = state.remaining.get_mut(replacement) else {
                        return failure("REEL_NOT_FOUND");
                    };
                    if *available == 0 {
                        return failure("MATERIAL_SHORTAGE");
                    }
                    *available -= 1;
                    board.replacements.push(replacement.into());
                    board.quality = Quality::AwaitingRetest;
                    board.history.push(
                        json!({"action":"rework","reel":replacement,"reason":args["reason"]}),
                    );
                }
                Action::SmtRetest if board.quality == Quality::AwaitingRetest => {
                    board.quality = if string("result") == "pass" && !recalled(reel) {
                        Quality::Clear
                    } else {
                        Quality::Held
                    };
                    board
                        .history
                        .push(json!({"action":"retest","result":args["result"]}));
                }
                Action::SmtScrap
                    if matches!(board.quality, Quality::Held | Quality::AwaitingRetest) =>
                {
                    board.quality = Quality::Scrapped;
                    board
                        .history
                        .push(json!({"action":"scrap","reason":args["reason"]}));
                }
                _ => return failure("INVALID_STATE"),
            }
            return json!({"ok":true,"serial":serial,"quality":board.quality,"finished":board.finished});
        }
    }
    json!({"ok":true})
}
