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
    #[serde(default)]
    fabrication: Option<crate::fabrication::Setup>,
    #[serde(default = "model_version")]
    model_version: u32,
    #[serde(default)]
    model_parameters: Map<String, Value>,
    #[serde(default)]
    event_rules: Vec<mom_des::Rule>,
    work_orders: Vec<String>,
    materials: BTreeMap<String, String>,
    capabilities: Vec<Action>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    receipt_reversals: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    operation_material_reversals: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    partial_lot_transfers: bool,
}

impl Definition {
    fn issued_report_inputs(&self) -> bool {
        self.manufacturing
            .as_ref()
            .is_some_and(|setup| setup.issued_report_inputs)
            || self
                .assembly
                .as_ref()
                .is_some_and(|setup| setup.issued_report_inputs)
    }
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

pub(crate) fn reject(code: &'static str, field: &'static str) -> Rejection {
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

pub(crate) fn identifier(value: &str) -> bool {
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
            setup
                .work_centers
                .as_ref()
                .is_none_or(|centers| centers.iter().all(|id| identifier(id))),
            "invalid manufacturing work center"
        );
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
        anyhow::ensure!(
            !setup.shared_material
                || (definition.capabilities.contains(&Action::GetOrderSnapshot)
                    && definition
                        .capabilities
                        .contains(&Action::IssueMaterialToOperation)
                    && !definition.capabilities.iter().any(|action| matches!(
                        action,
                        Action::SmtRelease
                            | Action::SmtRework
                            | Action::ReturnUnusedMaterial
                            | Action::ReverseInventoryMovement
                    ))),
            "shared SMT inventory requires frozen order release and placement-only consumption"
        );
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
    if let Some(setup) = definition.fabrication {
        anyhow::ensure!(
            setup.model() == definition.model,
            "fabrication setup requires its registered model"
        );
    }
    anyhow::ensure!(
        !definition.capabilities.contains(&Action::ReleaseWorkOrder)
            || definition.assembly.is_some()
            || definition.manufacturing.is_some()
            || definition
                .smt
                .as_ref()
                .is_some_and(|setup| setup.shared_material),
        "work order release requires assembly, manufacturing, or shared SMT setup"
    );
    anyhow::ensure!(
        (!definition.capabilities.contains(&Action::CreateWorkOrder)
            && !definition.capabilities.contains(&Action::UpdateDraftOrder))
            || (definition.capabilities.contains(&Action::CreateWorkOrder)
                && definition.capabilities.contains(&Action::ReleaseWorkOrder)),
        "draft capabilities require work order creation and release"
    );
    anyhow::ensure!(
        !definition.capabilities.contains(&Action::ReleaseWorkOrders)
            || definition.capabilities.contains(&Action::ReleaseWorkOrder),
        "batch release requires single work order release"
    );
    anyhow::ensure!(
        !definition
            .capabilities
            .iter()
            .any(|action| matches!(action, Action::HoldWorkOrder | Action::ResumeWorkOrder))
            || [
                Action::HoldWorkOrder,
                Action::ResumeWorkOrder,
                Action::ReleaseWorkOrder
            ]
            .iter()
            .all(|action| definition.capabilities.contains(action)),
        "work order hold and resume require both commands and work order release"
    );
    anyhow::ensure!(
        !definition.capabilities.iter().any(|action| matches!(
            action,
            Action::CancelWorkOrder | Action::CompleteWorkOrder | Action::CloseWorkOrder
        )) || (definition.capabilities.contains(&Action::ReleaseWorkOrder)
            && !definition.capabilities.contains(&Action::CloseOrder)),
        "terminal actions require work order release and exclude legacy close"
    );
    anyhow::ensure!(
        !definition.capabilities.iter().any(|action| matches!(
            action,
            Action::ListOperations | Action::GetOperation | Action::ListDispatchQueue
        )) || definition.assembly.is_some()
            || definition.manufacturing.is_some(),
        "operation queries require assembly or manufacturing setup"
    );
    anyhow::ensure!(
        !definition
            .capabilities
            .contains(&Action::SetDispatchPriority)
            || (definition.capabilities.contains(&Action::DispatchOperation)
                && (definition.assembly.is_some()
                    || definition.manufacturing.as_ref().is_some_and(|setup| setup
                        .strict_dispatch
                        && setup.work_centers.is_some()))),
        "shared dispatch requires assembly or configured manufacturing work centers"
    );
    anyhow::ensure!(
        !definition.capabilities.contains(&Action::ReassignOperation)
            || (definition.capabilities.contains(&Action::DispatchOperation)
                && (definition
                    .assembly
                    .as_ref()
                    .is_some_and(|setup| setup.exclusive_dispatch)
                    || definition.manufacturing.as_ref().is_some_and(|setup| setup
                        .strict_dispatch
                        && setup.work_centers.is_some()
                        && !setup.alternate_centers.is_empty()
                        && setup
                            .alternate_centers
                            .iter()
                            .all(|(center, alternates)| identifier(center)
                                && alternates.iter().all(|alternate| identifier(alternate)))))),
        "resource reassignment requires exclusive dispatch and valid work centers"
    );
    anyhow::ensure!(
        !definition.capabilities.iter().any(|action| matches!(
            action,
            Action::PostOperationReport
                | Action::ListOperationReports
                | Action::ReverseOperationReport
                | Action::CompleteOperation
        )) || ((definition.manufacturing.is_some() || definition.assembly.is_some())
            && !definition.capabilities.contains(&Action::AssemblyBuild)
            && !definition.capabilities.contains(&Action::ReportOperation)
            && !definition.capabilities.contains(&Action::ReverseReport)),
        "public operation actuals require manufacturing or assembly and exclude legacy write commands"
    );
    anyhow::ensure!(
        !definition
            .capabilities
            .contains(&Action::ReturnUnusedMaterial)
            || (definition.issued_report_inputs()
                && definition
                    .capabilities
                    .contains(&Action::IssueMaterialToOperation)
                && definition
                    .capabilities
                    .contains(&Action::PostOperationReport)),
        "operation material returns require recorded issue and report inputs"
    );
    anyhow::ensure!(
        !definition.operation_material_reversals
            || (definition.issued_report_inputs()
                && definition
                    .capabilities
                    .contains(&Action::ReturnUnusedMaterial)
                && definition
                    .capabilities
                    .contains(&Action::ReverseInventoryMovement)),
        "operation material reversals require recorded manufacturing inputs and returns"
    );
    anyhow::ensure!(
        !definition.capabilities.contains(&Action::GetWip)
            || (definition
                .manufacturing
                .as_ref()
                .is_some_and(|setup| setup.issued_report_inputs)
                && definition
                    .capabilities
                    .contains(&Action::PostOperationReport)),
        "work in process requires recorded operation output"
    );
    anyhow::ensure!(
        !definition
            .capabilities
            .iter()
            .any(|action| action.is_genealogy_action())
            || definition.capabilities.contains(&Action::MoveWip)
            || definition
                .smt
                .as_ref()
                .is_some_and(|setup| setup.shared_material),
        "genealogy requires recorded material inputs and work-in-process movement"
    );
    anyhow::ensure!(
        !definition
            .capabilities
            .contains(&Action::ReverseWipMovement)
            || definition.capabilities.contains(&Action::MoveWip),
        "work in process movement reversal requires work in process movement"
    );
    anyhow::ensure!(
        !definition.capabilities.iter().any(|action| matches!(
            action,
            Action::SplitWip | Action::MergeWip | Action::ConsolidateWip
        )) || [
            Action::SplitWip,
            Action::MergeWip,
            Action::ReverseWipMovement,
            Action::ListGenealogyEdges
        ]
        .iter()
        .all(|action| definition.capabilities.contains(action)),
        "work in process split and merge require movement reversal and genealogy"
    );
    anyhow::ensure!(
        definition
            .capabilities
            .contains(&Action::ReceiveWorkOrderOutput)
            == definition
                .manufacturing
                .as_ref()
                .is_some_and(|setup| setup.finished_output_receipt)
            && (!definition
                .capabilities
                .contains(&Action::ReceiveWorkOrderOutput)
                || [
                    Action::CompleteWorkOrder,
                    Action::CloseWorkOrder,
                    Action::MoveWip,
                    Action::GetLot,
                    Action::GetStockBalance,
                    Action::ListGenealogyEdges,
                    Action::ReverseInventoryMovement
                ]
                .iter()
                .all(|action| definition.capabilities.contains(action))
                    && !definition.capabilities.contains(&Action::CloseOrder)),
        "finished output requires recorded WIP, inventory, genealogy and controlled work-order closure"
    );
    anyhow::ensure!(
        !definition
            .capabilities
            .contains(&Action::ReconcileWorkOrder)
            || definition.capabilities.contains(&Action::MoveWip),
        "work order reconciliation requires work in process movement"
    );
    anyhow::ensure!(
        definition.capabilities.contains(&Action::MoveWip)
            == definition
                .manufacturing
                .as_ref()
                .is_some_and(|setup| setup.wip_movements),
        "work in process movement must be enabled with downstream input validation"
    );
    anyhow::ensure!(
        !definition.capabilities.contains(&Action::MoveWip)
            || (definition.capabilities.contains(&Action::GetWip)
                && definition.capabilities.contains(&Action::GetOrderSnapshot)),
        "work in process movement requires a frozen operation route"
    );

    anyhow::ensure!(
        !definition
            .capabilities
            .contains(&Action::RequestMaintenance)
            || (definition
                .manufacturing
                .as_ref()
                .is_some_and(|setup| setup.equipment_events)
                && definition.model == "cnc_wear"
                && definition.model_parameters.get("stop_on_alarm") == Some(&json!(true))
                && !definition.capabilities.contains(&Action::SetEquipment)),
        "maintenance requires event-driven CNC equipment"
    );
    let has = |action| definition.capabilities.contains(&action);
    anyhow::ensure!(
        !has(Action::AssessEngineeringChange) || has(Action::GetOrderSnapshot),
        "engineering change assessment requires order snapshots"
    );
    anyhow::ensure!(
        !has(Action::GetOrderSnapshot)
            || ([
                Action::GetReleaseReadiness,
                Action::ReleaseWorkOrder,
                Action::CreateBomRevision,
                Action::CreateRouteRevision,
                Action::ActivateInspectionPlanRevision
            ]
            .into_iter()
            .all(has)
                && !has(Action::ReleaseOrder)
                && !has(Action::ReleaseWorkOrders)),
        "order snapshots require versioned definitions and single-order release"
    );
    for action in &definition.capabilities {
        if crate::fabrication::tools().any(|tool| tool.0 == *action) {
            anyhow::ensure!(
                definition
                    .fabrication
                    .is_some_and(|setup| setup.valid(&definition.model, *action)),
                "fabrication capability does not match setup"
            );
        }
    }
    Ok(definition)
}

fn read_work_order(
    tx: &rusqlite::Transaction<'_>,
    run: &str,
    definition: &Definition,
    order: &str,
) -> anyhow::Result<Option<Value>> {
    if !definition.work_orders.iter().any(|id| id == order) {
        return Ok(None);
    }
    if let Some(setup) = &definition.assembly {
        return crate::assembly::status(tx, run, setup, order);
    }
    if let Some(setup) = &definition.smt {
        let status = crate::smt::work_order_status(tx, run, setup, order)?;
        return Ok(Some(
            json!({"ok":true,"run_id":run,"work_order":order,"status":status}),
        ));
    }
    if let Some(setup) = &definition.manufacturing {
        return crate::mes::order_status(tx, run, setup, &definition.work_orders, order);
    }
    Ok(Some(
        json!({"ok":true,"run_id":run,"work_order":order,"status":"planned"}),
    ))
}

impl Scenarios {
    pub fn new(db: PathBuf) -> anyhow::Result<Self> {
        let store = Self { db };
        let mut conn = store.connect()?;
        // Commit the schema and built-in fixtures together before accepting requests.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(
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
             CREATE TABLE IF NOT EXISTS fabrication_runs (run TEXT PRIMARY KEY, state TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS assembly_runs (run TEXT PRIMARY KEY, state TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS smt_runs (run TEXT PRIMARY KEY, state TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS mes_runs (run TEXT PRIMARY KEY, state TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS des_runs (run TEXT PRIMARY KEY, state TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS scenario_created_orders (
                 run TEXT NOT NULL, id TEXT NOT NULL, template TEXT NOT NULL,
                 demand TEXT NOT NULL, serial TEXT,
                 PRIMARY KEY (run, id)
             );
             CREATE TABLE IF NOT EXISTS stock_receipt_inspections (
                 run TEXT NOT NULL, inspection TEXT NOT NULL, receipt TEXT NOT NULL,
                 PRIMARY KEY (run,inspection)
             );
             CREATE TABLE IF NOT EXISTS stock_receipt_releases (
                 run TEXT NOT NULL, receipt TEXT NOT NULL, inspection TEXT NOT NULL,
                 released_cursor INTEGER NOT NULL,
                 PRIMARY KEY (run,receipt)
             );
             CREATE TABLE IF NOT EXISTS stock_operation_issues (
                 run TEXT NOT NULL, reservation TEXT NOT NULL,
                 operation TEXT NOT NULL, work_center TEXT NOT NULL, issued_cursor INTEGER NOT NULL,
                 PRIMARY KEY(run,reservation)
             );
             CREATE TABLE IF NOT EXISTS stock_transfers (
                 run TEXT NOT NULL, transfer TEXT NOT NULL, lot TEXT NOT NULL,
                 source TEXT NOT NULL, destination TEXT NOT NULL, reason TEXT NOT NULL,
                 posted_cursor INTEGER, quantity INTEGER,
                 PRIMARY KEY(run,transfer)
             );
             CREATE TABLE IF NOT EXISTS stock_lot_splits (
                 run TEXT NOT NULL, transfer TEXT NOT NULL, child TEXT NOT NULL, amount INTEGER NOT NULL,
                 PRIMARY KEY(run,transfer), UNIQUE(run,child)
             );
             CREATE VIEW IF NOT EXISTS stock_lot_origins AS
                 SELECT run,receipt,item,revision,unit,quantity,requires_inspection,lot,supplier_lot,location,posted_cursor,
                     NULL AS parent_lot FROM stock_receipts
                 UNION ALL
                 SELECT t.run,s.receipt,s.item,s.revision,s.unit,p.amount,s.requires_inspection,p.child,s.supplier_lot,
                     t.destination,s.posted_cursor,s.lot
                 FROM stock_lot_splits p JOIN stock_transfers t ON t.run=p.run AND t.transfer=p.transfer
                 JOIN stock_receipts s ON s.run=t.run AND s.lot=t.lot WHERE t.posted_cursor IS NOT NULL;
             CREATE VIEW IF NOT EXISTS stock_position_lots AS
                 SELECT o.run,o.receipt,o.item,o.revision,o.unit,
                     o.quantity-COALESCE((SELECT SUM(p.amount) FROM stock_lot_splits p
                       JOIN stock_transfers t ON t.run=p.run AND t.transfer=p.transfer
                       WHERE t.run=o.run AND t.lot=o.lot AND t.posted_cursor IS NOT NULL
                       AND NOT EXISTS(SELECT 1 FROM stock_transfer_reversals r WHERE r.run=t.run AND r.original=t.transfer)),0) AS quantity,
                     o.requires_inspection,o.lot,o.supplier_lot,
                     COALESCE((SELECT t.destination FROM stock_whole_transfers t WHERE t.run=o.run AND t.lot=o.lot
                       AND t.posted_cursor IS NOT NULL
                       ORDER BY t.posted_cursor DESC LIMIT 1),o.location) AS location,o.posted_cursor,o.parent_lot
                 FROM stock_lot_origins o;
             CREATE VIEW IF NOT EXISTS stock_available_lots AS
                 SELECT s.* FROM stock_position_lots s WHERE NOT EXISTS(
                     SELECT 1 FROM stock_receipt_reversals r WHERE r.run=s.run AND r.receipt=s.receipt)
                 AND NOT EXISTS(SELECT 1 FROM stock_lot_splits p JOIN stock_transfer_reversals r
                     ON r.run=p.run AND r.original=p.transfer WHERE p.run=s.run AND p.child=s.lot);
             CREATE VIEW IF NOT EXISTS stock_whole_transfers AS
                 SELECT t.* FROM stock_transfers t WHERE NOT EXISTS(SELECT 1 FROM stock_lot_splits p
                   WHERE p.run=t.run AND (p.transfer=t.transfer OR EXISTS(SELECT 1 FROM stock_transfer_reversals r
                      WHERE r.run=t.run AND r.reversal=t.transfer AND r.original=p.transfer)));
             CREATE TABLE IF NOT EXISTS stock_quality_holds (
                 run TEXT NOT NULL, hold TEXT NOT NULL, lot TEXT NOT NULL,
                 reason TEXT NOT NULL, held_cursor INTEGER NOT NULL,
                 release_reason TEXT, released_cursor INTEGER,
                 PRIMARY KEY(run,hold)
             );
             CREATE UNIQUE INDEX IF NOT EXISTS active_stock_quality_hold ON stock_quality_holds(run,lot)
                 WHERE released_cursor IS NULL;
             CREATE VIEW IF NOT EXISTS stock_scoped_lot_holds AS
                 SELECT run,lot AS batch FROM stock_quality_holds WHERE released_cursor IS NULL
                 UNION
                 SELECT o.run,o.lot FROM stock_lot_origins o JOIN stock_quality_holds h
                     ON h.run=o.run AND h.lot=o.parent_lot WHERE h.released_cursor IS NULL;
             CREATE VIEW IF NOT EXISTS stock_effective_lot_holds AS
                 SELECT run,batch FROM stock_lot_holds UNION SELECT run,batch FROM stock_scoped_lot_holds;
             CREATE VIEW IF NOT EXISTS stock_lot_holds AS
                 SELECT run,batch FROM scenario_holds
                 UNION
                 SELECT o.run,o.lot FROM stock_lot_origins o JOIN scenario_holds h
                     ON h.run=o.run AND h.batch=o.parent_lot;
             CREATE TABLE IF NOT EXISTS stock_receipt_reversals (
                 run TEXT NOT NULL, receipt TEXT NOT NULL, reversal TEXT NOT NULL,
                 reason TEXT NOT NULL, reversed_cursor INTEGER NOT NULL,
                 PRIMARY KEY(run,receipt), UNIQUE(run,reversal)
             );
             CREATE TABLE IF NOT EXISTS stock_transfer_reversals (
                 run TEXT NOT NULL, original TEXT NOT NULL, reversal TEXT NOT NULL,
                 PRIMARY KEY(run,original), UNIQUE(run,reversal)
             );
             CREATE INDEX IF NOT EXISTS stock_transfer_history ON stock_transfers(run,lot,posted_cursor);
             CREATE TABLE IF NOT EXISTS stock_operation_returns (
                 run TEXT NOT NULL, movement TEXT NOT NULL, reservation TEXT NOT NULL,
                 quantity INTEGER NOT NULL, reason TEXT NOT NULL, returned_cursor INTEGER NOT NULL,
                 PRIMARY KEY(run,movement)
             );
             CREATE INDEX IF NOT EXISTS stock_return_reservation ON stock_operation_returns(run,reservation);
             CREATE TABLE IF NOT EXISTS stock_operation_reversals (
                 run TEXT NOT NULL, kind TEXT NOT NULL, original TEXT NOT NULL, reversal TEXT NOT NULL,
                 reservation TEXT NOT NULL, lot TEXT NOT NULL, location TEXT NOT NULL,
                 quantity INTEGER NOT NULL, reason TEXT NOT NULL, reversed_cursor INTEGER NOT NULL,
                 PRIMARY KEY(run,kind,original), UNIQUE(run,reversal)
             );
             CREATE VIEW IF NOT EXISTS stock_effective_return_totals AS
                 SELECT t.run,t.reservation,SUM(t.quantity) AS quantity FROM stock_operation_returns t
                 WHERE NOT EXISTS(SELECT 1 FROM stock_operation_reversals r
                     WHERE r.run=t.run AND r.kind='return' AND r.original=t.movement)
                 GROUP BY t.run,t.reservation;
             CREATE TABLE IF NOT EXISTS stock_reservations (
                 run TEXT NOT NULL, reservation TEXT NOT NULL, work_order TEXT NOT NULL,
                 lot TEXT NOT NULL, quantity INTEGER NOT NULL, status TEXT NOT NULL,
                 PRIMARY KEY(run,reservation)
             );
             CREATE INDEX IF NOT EXISTS stock_reservation_lot ON stock_reservations (run,lot,status);
             CREATE INDEX IF NOT EXISTS stock_reservation_order ON stock_reservations (run,work_order,status);
             CREATE TABLE IF NOT EXISTS stock_receipts (
                 run TEXT NOT NULL, receipt TEXT NOT NULL, item TEXT NOT NULL,
                 revision INTEGER NOT NULL, unit TEXT NOT NULL, quantity INTEGER NOT NULL,
                 requires_inspection INTEGER NOT NULL, lot TEXT, supplier_lot TEXT,
                 location TEXT, posted_cursor INTEGER,
                 PRIMARY KEY (run,receipt), UNIQUE (run,lot)
             );
             CREATE TABLE IF NOT EXISTS stock_work_order_output (
                 run TEXT NOT NULL, receipt TEXT NOT NULL, work_order TEXT NOT NULL,
                 serial_numbers TEXT NOT NULL, settled INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY(run,receipt)
             );
             CREATE INDEX IF NOT EXISTS stock_work_order_output_order ON stock_work_order_output(run,work_order);
             CREATE TABLE IF NOT EXISTS order_definition_snapshots (
                 run TEXT NOT NULL, work_order TEXT NOT NULL, snapshot TEXT NOT NULL,
                 PRIMARY KEY(run,work_order)
             );
             CREATE TABLE IF NOT EXISTS master_routes (
                 run TEXT NOT NULL, item TEXT NOT NULL, revision INTEGER NOT NULL,
                 operations TEXT NOT NULL, status TEXT NOT NULL,
                 PRIMARY KEY (run,item,revision)
             );
             CREATE UNIQUE INDEX IF NOT EXISTS master_active_route
                 ON master_routes (run,item) WHERE status='active';
             CREATE TABLE IF NOT EXISTS master_boms (
                 run TEXT NOT NULL, item TEXT NOT NULL, revision INTEGER NOT NULL,
                 components TEXT NOT NULL, status TEXT NOT NULL,
                 PRIMARY KEY (run,item,revision)
             );
             CREATE UNIQUE INDEX IF NOT EXISTS master_active_bom
                 ON master_boms (run,item) WHERE status='active';
             CREATE TABLE IF NOT EXISTS master_items (
                 run TEXT NOT NULL, item TEXT NOT NULL, unit TEXT NOT NULL,
                 PRIMARY KEY (run,item)
             );
             CREATE TABLE IF NOT EXISTS master_item_revisions (
                 run TEXT NOT NULL, item TEXT NOT NULL, revision INTEGER NOT NULL,
                 name TEXT NOT NULL, status TEXT NOT NULL,
                 PRIMARY KEY (run,item,revision)
             );
             CREATE UNIQUE INDEX IF NOT EXISTS master_item_active_revision
                 ON master_item_revisions (run,item) WHERE status='active';
             CREATE TABLE IF NOT EXISTS quality_inspection_plans (
                 run TEXT NOT NULL, plan TEXT NOT NULL, revision INTEGER NOT NULL,
                 specification TEXT NOT NULL, characteristics TEXT NOT NULL, status TEXT NOT NULL,
                 PRIMARY KEY (run, plan, revision), UNIQUE (run, specification)
             );
             CREATE UNIQUE INDEX IF NOT EXISTS quality_active_plan
                 ON quality_inspection_plans (run,plan) WHERE status='active';
             CREATE TABLE IF NOT EXISTS quality_inspections (
                 run TEXT NOT NULL, id TEXT NOT NULL, work_order TEXT NOT NULL,
                 specification_version TEXT NOT NULL,
                 PRIMARY KEY (run, id)
             );
             CREATE TABLE IF NOT EXISTS quality_measurements (
                 run TEXT NOT NULL, inspection TEXT NOT NULL, characteristic TEXT NOT NULL,
                 value REAL NOT NULL, unit TEXT NOT NULL, lower_limit REAL NOT NULL,
                 upper_limit REAL NOT NULL, passed INTEGER NOT NULL,
                 PRIMARY KEY (run, inspection, characteristic)
             );
             CREATE TABLE IF NOT EXISTS quality_inspection_results (
                 run TEXT NOT NULL, inspection TEXT NOT NULL, status TEXT NOT NULL,
                 PRIMARY KEY (run, inspection)
             );
             CREATE TABLE IF NOT EXISTS quality_nonconformances (
                 run TEXT NOT NULL, inspection TEXT NOT NULL, state TEXT NOT NULL,
                 PRIMARY KEY (run, inspection)
             );
             CREATE TABLE IF NOT EXISTS scenario_requests (
                 run TEXT NOT NULL, key TEXT NOT NULL, request TEXT NOT NULL, response TEXT NOT NULL,
                 PRIMARY KEY (run, key)
             );"
        )?;
        for source in [
            include_str!("../fixtures/cnc.json"),
            include_str!("../fixtures/cnc-manufacturing.json"),
            include_str!("../fixtures/cnc-public-orders.json"),
            include_str!("../fixtures/cnc-order-release.json"),
            include_str!("../fixtures/cnc-order-drafts.json"),
            include_str!("../fixtures/cnc-batch-release.json"),
            include_str!("../fixtures/cnc-order-hold.json"),
            include_str!("../fixtures/cnc-order-terminal.json"),
            include_str!("../fixtures/cnc-cross-shop.json"),
            include_str!("../fixtures/cnc-operation-reads.json"),
            include_str!("../fixtures/cnc-resource-dispatch.json"),
            include_str!("../fixtures/cnc-resource-occupancy.json"),
            include_str!("../fixtures/cnc-operation-lifecycle.json"),
            include_str!("../fixtures/cnc-operation-reports.json"),
            include_str!("../fixtures/cnc-des-recovery.json"),
            include_str!("../fixtures/cnc-inspections.json"),
            include_str!("../fixtures/assembly-inspections.json"),
            include_str!("../fixtures/cnc-inspection-results.json"),
            include_str!("../fixtures/assembly-inspection-results.json"),
            include_str!("../fixtures/cnc-quality-gates.json"),
            include_str!("../fixtures/assembly-quality-gates.json"),
            include_str!("../fixtures/cnc-inspection-plans.json"),
            include_str!("../fixtures/assembly-inspection-plans.json"),
            include_str!("../fixtures/cnc-item-revisions.json"),
            include_str!("../fixtures/assembly-item-revisions.json"),
            include_str!("../fixtures/cnc-bom-revisions.json"),
            include_str!("../fixtures/assembly-bom-revisions.json"),
            include_str!("../fixtures/cnc-route-revisions.json"),
            include_str!("../fixtures/assembly-route-revisions.json"),
            include_str!("../fixtures/cnc-order-snapshots.json"),
            include_str!("../fixtures/assembly-order-snapshots.json"),
            include_str!("../fixtures/cnc-receipts.json"),
            include_str!("../fixtures/assembly-receipts.json"),
            include_str!("../fixtures/cnc-receipt-quality.json"),
            include_str!("../fixtures/assembly-receipt-quality.json"),
            include_str!("../fixtures/cnc-reservations.json"),
            include_str!("../fixtures/assembly-reservations.json"),
            include_str!("../fixtures/cnc-operation-issues.json"),
            include_str!("../fixtures/cnc-report-inputs.json"),
            include_str!("../fixtures/cnc-unused-material-returns.json"),
            include_str!("../fixtures/cnc-stock-transfers.json"),
            include_str!("../fixtures/cnc-stock-transfer-reversal.json"),
            include_str!("../fixtures/cnc-receipt-reversal.json"),
            include_str!("../fixtures/cnc-operation-material-reversals.json"),
            include_str!("../fixtures/cnc-partial-lot-transfers.json"),
            include_str!("../fixtures/cnc-wip-output.json"),
            include_str!("../fixtures/cnc-wip-movement.json"),
            include_str!("../fixtures/cnc-wip-genealogy.json"),
            include_str!("../fixtures/cnc-wip-movement-reversal.json"),
            include_str!("../fixtures/cnc-wip-split-merge.json"),
            include_str!("../fixtures/cnc-finished-output-receipt.json"),
            include_str!("../fixtures/cnc-work-order-reconciliation.json"),
            include_str!("../fixtures/cnc-full-order-reconciliation.json"),
            include_str!("../fixtures/cnc-lot-quality-scope.json"),
            include_str!("../fixtures/cnc-cross-source-wip.json"),
            include_str!("../fixtures/assembly-operation-issues.json"),
            include_str!("../fixtures/assembly-materials.json"),
            include_str!("../fixtures/assembly-partial-lot-transfers.json"),
            include_str!("../fixtures/assembly-operation-reports.json"),
            include_str!("../fixtures/assembly-operation-lifecycle.json"),
            include_str!("../fixtures/assembly-resource-occupancy.json"),
            include_str!("../fixtures/assembly-resource-dispatch.json"),
            include_str!("../fixtures/assembly-operation-reads.json"),
            include_str!("../fixtures/assembly-order-terminal.json"),
            include_str!("../fixtures/assembly-order-hold.json"),
            include_str!("../fixtures/assembly-batch-release.json"),
            include_str!("../fixtures/assembly-shortage-batch-release.json"),
            include_str!("../fixtures/assembly-order-drafts.json"),
            include_str!("../fixtures/assembly-order-release.json"),
            include_str!("../fixtures/assembly-shortage-order-release.json"),
            include_str!("../fixtures/cnc-shortage.json"),
            include_str!("../fixtures/cnc-lot.json"),
            include_str!("../fixtures/smt.json"),
            include_str!("../fixtures/smt-production.json"),
            include_str!("../fixtures/smt-shared-ledger.json"),
            include_str!("../fixtures/assembly.json"),
            include_str!("../fixtures/assembly-production.json"),
            include_str!("../fixtures/assembly-public-orders.json"),
            include_str!("../fixtures/assembly-shortage.json"),
            include_str!("../fixtures/assembly-release-readiness.json"),
            include_str!("../fixtures/assembly-shortage-readiness.json"),
            include_str!("../fixtures/sheet-metal.json"),
            include_str!("../fixtures/wire-harness.json"),
        ] {
            let definition = parse_definition(source)?;
            tx.execute(
                "INSERT OR IGNORE INTO scenario_definitions VALUES (?1,?2,?3)",
                params![definition.scenario_id, definition.version, source],
            )?;
        }
        tx.commit()?;
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
        if let Err(error) = catalog::validate(&tool.input_schema(control), &args) {
            return Ok(error.json());
        }
        match self.dispatch(name, &args, control)? {
            Ok(value) => Ok(value),
            Err(error) => Ok(error.json()),
        }
    }

    fn dispatch(
        &self,
        name: &str,
        args: &Map<String, Value>,
        control: bool,
    ) -> anyhow::Result<Result<Value, Rejection>> {
        let mut conn = self.connect()?;
        if name == "list_scenarios" {
            let mut query =
                conn.prepare("SELECT definition FROM scenario_definitions ORDER BY id, version")?;
            let mut scenarios = Vec::new();
            for row in query.query_map([], |r| r.get::<_, String>(0))? {
                let spec: Definition = serde_json::from_str(&row?)?;
                let mut scenario = json!({"scenario_id":spec.scenario_id,"version":spec.version,
                    "title":spec.title,"capabilities":spec.capabilities});
                if control {
                    scenario["model"] = spec.model.into();
                    scenario["model_version"] = spec.model_version.into();
                }
                scenarios.push(scenario);
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
                return Ok(Err(reject("SCENARIO_CONFIGURATION_INVALID", "scenario_id")));
            }
            let id: String = conn.query_row(
                "INSERT INTO scenario_runs (id,scenario,version,seed)
                 VALUES (lower(hex(randomblob(16))),?1,?2,?3) RETURNING id",
                params![scenario, version, seed],
                |row| row.get(0),
            )?;
            return Ok(Ok(run_view(&id, scenario, version, false, 0, None)));
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
                |state| state.page(after, limit, control),
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
        let mut definition: Definition = serde_json::from_str(&source)?;
        let mut created = BTreeMap::new();
        if definition.capabilities.contains(&Action::CreateWorkOrder) {
            let mut statement = tx.prepare(
                "SELECT id,template,demand,serial FROM scenario_created_orders WHERE run=?1",
            )?;
            let mut rows = statement.query([run])?;
            while let Some(row) = rows.next()? {
                let id: String = row.get(0)?;
                let template: String = row.get(1)?;
                let demand: String = row.get(2)?;
                if let Some(setup) = &mut definition.assembly {
                    let serial: String = row.get(3)?;
                    setup
                        .add_order(&id, &template, &serial)
                        .map_err(anyhow::Error::msg)?;
                }
                definition.work_orders.push(id.clone());
                created.insert(id, (template, demand));
            }
        }
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
                crate::mes::project_equipment(
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
        let bom_enabled = definition.capabilities.contains(&Action::CreateBomRevision);
        let route_enabled = definition
            .capabilities
            .contains(&Action::CreateRouteRevision);
        let snapshot_enabled = definition.capabilities.contains(&Action::GetOrderSnapshot);
        let mut spec = match action {
            Action::GetReleaseReadiness | Action::ReleaseWorkOrder if snapshot_enabled => {
                crate::snapshots::release_spec(action)
            }
            Action::ReverseInventoryMovement if definition.operation_material_reversals => {
                crate::operation_reversals::extend_spec(crate::receipt_reversals::extend_spec(
                    action.spec(),
                ))
            }
            Action::ReverseInventoryMovement if definition.receipt_reversals => {
                crate::receipt_reversals::extend_spec(action.spec())
            }
            Action::CreateStockTransfer if definition.partial_lot_transfers => {
                crate::transfers::partial_spec()
            }
            Action::ActivateRevision => crate::items::spec(action, bom_enabled, route_enabled),
            Action::ValidateRevision => crate::bom::validation_spec(route_enabled),
            _ => action.spec(),
        };
        if action == Action::CreateInspection
            && definition
                .capabilities
                .contains(&Action::ReleaseReceiptInspection)
        {
            spec.input["properties"]["receipt_id"] = catalog::text();
        }
        if action == Action::PostOperationReport && !definition.issued_report_inputs() {
            spec.input["properties"]
                .as_object_mut()
                .unwrap()
                .remove("material_inputs");
        }
        if action == Action::PostOperationReport
            && definition.capabilities.contains(&Action::MoveWip)
        {
            spec.input["properties"]["wip_inputs"] = crate::wip::inputs_schema();
        }
        let shared_smt = definition
            .smt
            .as_ref()
            .filter(|setup| setup.shared_material);
        if action == Action::SmtPlace && shared_smt.is_some() {
            spec = crate::smt::placement_spec(spec);
        }
        if name == "describe_tool" {
            return Ok(Ok(json!({"ok":true,"tool":spec})));
        }
        let parameters = args["arguments"].as_object().unwrap();
        if let Err(error) = catalog::validate(&spec.input, parameters) {
            return Ok(Err(error));
        }
        if matches!(
            action,
            Action::GetOrderSnapshot | Action::AssessEngineeringChange
        ) {
            return crate::snapshots::query(
                &tx,
                run,
                action,
                parameters["work_order"].as_str().unwrap(),
            );
        }
        let mut definition_snapshot = None;
        if snapshot_enabled
            && matches!(
                action,
                Action::GetReleaseReadiness | Action::ReleaseWorkOrder
            )
        {
            let order = parameters["work_order"].as_str().unwrap();
            if !definition.work_orders.iter().any(|id| id == order) {
                return Ok(Err(reject("WORK_ORDER_NOT_FOUND", "work_order")));
            }
            if action == Action::ReleaseWorkOrder {
                let key = parameters["idempotency_key"].as_str().unwrap();
                let request = json!({"action":action,"arguments":parameters}).to_string();
                let saved = tx
                    .query_row(
                        "SELECT request,response FROM scenario_requests WHERE run=?1 AND key=?2",
                        params![run, key],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()?;
                if let Some((original, response)) = saved {
                    return Ok(if original == request {
                        Ok(serde_json::from_str(&response)?)
                    } else {
                        Err(reject("IDEMPOTENCY_CONFLICT", "idempotency_key"))
                    });
                }
            }
            let selection = match crate::snapshots::selection(&parameters["definition_selection"]) {
                Ok(selection) => selection,
                Err(error) => return Ok(Err(error)),
            };
            let resolved = crate::snapshots::resolve(&tx, run, order, &selection)?;
            match (action, resolved) {
                (Action::ReleaseWorkOrder, Ok(snapshot))
                    if snapshot["fingerprint"] == parameters["snapshot_fingerprint"] =>
                {
                    definition_snapshot = Some(snapshot);
                }
                (Action::ReleaseWorkOrder, _) => {
                    return Ok(Err(reject("SNAPSHOT_CHANGED", "snapshot_fingerprint")));
                }
                (_, Ok(snapshot)) => definition_snapshot = Some(snapshot),
                (_, Err(error)) => return Ok(Err(error)),
            }
        }
        if matches!(action, Action::HoldStockLot | Action::ReleaseStockLotHold) {
            return crate::lot_quality::invoke(tx, run, action, parameters, cursor);
        }
        if action.is_reservation_action() {
            let order = match action {
                Action::ReserveMaterial => {
                    Some(parameters["work_order"].as_str().unwrap().to_owned())
                }
                Action::PickMaterial | Action::IssueMaterialToOperation => tx
                    .query_row(
                        "SELECT work_order FROM stock_reservations WHERE run=?1 AND reservation=?2",
                        params![run, parameters["reservation_id"].as_str().unwrap()],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?,
                _ => None,
            };
            let quantity = if let (Some(setup), Some(order)) = (shared_smt, order.as_deref()) {
                crate::smt::planned_units(setup, order)
            } else if definition.manufacturing.is_some() {
                crate::mes::QUANTITY as i64
            } else {
                1
            };
            let eligible = if let Some(order) = &order {
                read_work_order(&tx, run, &definition, order)?.is_some_and(|order| {
                    order["completion_confirmed"] != true
                        && !(shared_smt.is_some() && order["status"] == "complete")
                        && !matches!(
                            order["status"].as_str(),
                            Some("planned" | "cancelled" | "closed" | "completed")
                        )
                })
            } else {
                true
            };
            let issue = if action == Action::IssueMaterialToOperation
                && let Some(order) = &order
            {
                let operation = parameters["operation"].as_str().unwrap();
                let rows = if let Some(setup) = &definition.assembly {
                    crate::assembly::operations(&tx, run, setup, Some(order))?
                } else if let Some(setup) = &definition.manufacturing {
                    crate::mes::operations(&tx, run, setup, &definition.work_orders, Some(order))?
                } else {
                    Vec::new()
                };
                let snapshot = crate::snapshots::load(&tx, run, order)?;
                let center = snapshot.as_ref().and_then(|snapshot| {
                    snapshot["route"]["operations"][operation]["work_center"].as_str()
                });
                let actual = rows.iter().find(|row| row.operation == operation);
                if let Some(code) = crate::quality::blocking(&tx, run, order)? {
                    Err(if code == "QUALITY_SCRAPPED" {
                        "QUALITY_SCRAPPED"
                    } else {
                        "QUALITY_HOLD"
                    })
                } else if let Some(setup) = shared_smt {
                    if operation != crate::smt::PLACEMENT || center != Some(crate::smt::WORK_CENTER)
                    {
                        Err("ISSUE_OPERATION_NOT_FOUND")
                    } else if crate::smt::can_issue(&tx, run, setup, order)? {
                        Ok(Some(crate::smt::WORK_CENTER.to_owned()))
                    } else {
                        Err("ISSUE_OPERATION_NOT_ELIGIBLE")
                    }
                } else if center.is_none() || actual.is_none() {
                    Err("ISSUE_OPERATION_NOT_FOUND")
                } else if definition
                    .manufacturing
                    .as_ref()
                    .is_some_and(|setup| setup.issued_report_inputs)
                    && operation != crate::mes::MATERIAL_INPUT_OPERATION
                {
                    Err("ISSUE_OPERATION_NOT_ELIGIBLE")
                } else if definition
                    .assembly
                    .as_ref()
                    .is_some_and(|setup| setup.issued_report_inputs)
                    && operation != "build"
                {
                    Err("ISSUE_OPERATION_NOT_ELIGIBLE")
                } else if let Some(actual) = actual
                    && matches!(actual.status, "dispatched" | "running")
                    && actual.blocking.is_none()
                    && actual.work_center.as_deref() == center
                {
                    Ok(actual.work_center.clone())
                } else {
                    Err("ISSUE_OPERATION_NOT_ELIGIBLE")
                }
            } else {
                Ok(None)
            };
            return crate::reservations::invoke(
                tx, run, action, parameters, cursor, quantity, eligible, issue,
            );
        }
        if action == Action::GetMaterialRequirements {
            let order = parameters["work_order"].as_str().unwrap();
            let quantity = if let Some(setup) = shared_smt {
                crate::smt::planned_units(setup, order)
            } else if definition.manufacturing.is_some() {
                crate::mes::QUANTITY as i64
            } else {
                1
            };
            return crate::inventory::requirements(&tx, run, order, quantity);
        }
        if matches!(
            action,
            Action::CreateStockTransfer
                | Action::PostStockTransfer
                | Action::ReverseInventoryMovement
        ) {
            let order_quantity = if definition.assembly.is_some() {
                1
            } else {
                crate::mes::QUANTITY as i64
            };
            return crate::transfers::invoke(tx, run, action, parameters, cursor, order_quantity);
        }
        if action.is_receipt_action() {
            return crate::inventory::invoke(tx, run, action, parameters, cursor);
        }
        if action.is_route_action()
            || (matches!(action, Action::ActivateRevision | Action::ValidateRevision)
                && parameters["revision_type"] == "route")
        {
            return crate::routes::invoke(tx, run, action, parameters, cursor);
        }
        if action.is_bom_action()
            || (action == Action::ActivateRevision && parameters["revision_type"] == "bom")
        {
            if !definition.capabilities.contains(&Action::CreateBomRevision) {
                return Ok(Err(reject("TOOL_UNAVAILABLE", "name")));
            }
            return crate::bom::invoke(tx, run, action, parameters, cursor);
        }
        if action.is_item_action() {
            return crate::items::invoke(tx, run, action, parameters, cursor);
        }
        if matches!(
            action,
            Action::CreateInspectionPlanRevision
                | Action::ActivateInspectionPlanRevision
                | Action::CreateInspection
                | Action::ListInspections
                | Action::GetInspection
                | Action::RecordMeasurement
                | Action::CompleteInspection
                | Action::OpenNonconformance
                | Action::PlaceQualityHold
                | Action::DispositionNonconformance
                | Action::ReleaseQualityHold
        ) {
            return crate::quality::invoke(
                tx,
                run,
                &definition.work_orders,
                action,
                parameters,
                cursor,
                definition
                    .capabilities
                    .contains(&Action::ActivateInspectionPlanRevision),
            );
        }
        if definition
            .capabilities
            .contains(&Action::OpenNonconformance)
            && (matches!(
                action,
                Action::CompleteOperation
                    | Action::CompleteWorkOrder
                    | Action::CloseWorkOrder
                    | Action::CloseOrder
                    | Action::AssemblyDeliver
            ) || (matches!(action, Action::InspectUnits | Action::Disposition)
                && parameters["result"] == "pass"))
            && let Some(order) = parameters["work_order"].as_str()
            && let Some(code) = crate::quality::blocking(&tx, run, order)?
        {
            let key = parameters["idempotency_key"].as_str().unwrap();
            let has_receipt: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM scenario_requests WHERE run=?1 AND key=?2)",
                params![run, key],
                |row| row.get(0),
            )?;
            // A retry or key conflict is resolved by the write handler without a new mutation.
            if !has_receipt {
                return Ok(Ok(json!({"ok":false,"code":code,"field":"work_order"})));
            }
        }
        if action == Action::RequestMaintenance {
            let Some(setup) = definition
                .manufacturing
                .as_ref()
                .filter(|setup| setup.equipment_events)
            else {
                return Ok(Err(reject("TOOL_UNAVAILABLE", "name")));
            };
            if parameters["asset"] != "CNC-1" {
                return Ok(Err(reject("ASSET_NOT_FOUND", "asset")));
            }
            let key = parameters["idempotency_key"].as_str().unwrap();
            let request = json!({"action":action,"arguments":parameters}).to_string();
            let saved: Option<(String, String)> = tx
                .query_row(
                    "SELECT request,response FROM scenario_requests WHERE run=?1 AND key=?2",
                    params![run, key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((original, response)) = saved {
                return Ok(if original == request {
                    Ok(serde_json::from_str(&response)?)
                } else {
                    Err(reject("IDEMPOTENCY_CONFLICT", "idempotency_key"))
                });
            }
            if parameters["expected_cursor"].as_i64() != Some(cursor) {
                return Ok(Err(reject("VERSION_CONFLICT", "expected_cursor")));
            }
            if let Err(code) =
                crate::mes::request_maintenance(&tx, run, setup, &definition.work_orders)?
            {
                return Ok(Err(reject(code, "asset")));
            }
            let model = simulation::load_model(
                &definition.model,
                definition.model_version,
                &definition.model_parameters,
            )
            .map_err(anyhow::Error::msg)?;
            let mut state = stored.unwrap_or_else(|| {
                Simulation::new(
                    seed.parse().expect("stored seed"),
                    &definition.event_rules,
                    model.as_ref(),
                )
            });
            if let Err(code) = state.execute(
                Command::Intervene {
                    action: "maintenance".into(),
                },
                &definition.event_rules,
                model.as_ref(),
            ) {
                return Ok(Err(reject(code, "asset")));
            }
            tx.execute("INSERT INTO des_runs (run,state) VALUES (?1,?2) ON CONFLICT(run) DO UPDATE SET state=excluded.state",
                params![run,serde_json::to_string(&state)?])?;
            let response = json!({"ok":true,"run_id":run,"asset":"CNC-1","status":"maintenance_requested","cursor":cursor+1});
            tx.execute(
                "UPDATE scenario_runs SET cursor=cursor+1 WHERE id=?1",
                [run],
            )?;
            tx.execute(
                "INSERT INTO scenario_requests VALUES (?1,?2,?3,?4)",
                params![run, key, request, response.to_string()],
            )?;
            tx.commit()?;
            return Ok(Ok(response));
        }
        if action == Action::GetCommandResult {
            let key = parameters["idempotency_key"].as_str().unwrap();
            let saved: Option<String> = tx
                .query_row(
                    "SELECT response FROM scenario_requests WHERE run=?1 AND key=?2",
                    params![run, key],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(saved) = saved else {
                return Ok(Err(reject("COMMAND_RESULT_NOT_FOUND", "idempotency_key")));
            };
            let receipt: Value = serde_json::from_str(&saved)?;
            return Ok(Ok(json!({"ok":true,"run_id":run,"receipt":receipt})));
        }
        if action == Action::GetResourceCapability {
            let center = parameters["work_center"].as_str().unwrap();
            let result = if let Some(setup) = &definition.assembly {
                crate::assembly::resource_capability(&tx, run, setup, center)?
            } else if let Some(setup) = &definition.manufacturing {
                crate::mes::resource_capability(&tx, run, setup, &definition.work_orders, center)?
            } else {
                return Ok(Err(reject("TOOL_UNAVAILABLE", "name")));
            };
            return Ok(result
                .map(|resource| json!({"ok":true,"run_id":run,"resource":resource}))
                .ok_or_else(|| reject("RESOURCE_NOT_FOUND", "work_center")));
        }
        if action == Action::StartOperation
            && definition.capabilities.contains(&Action::PauseOperation)
            && !parameters.contains_key("expected_cursor")
        {
            return Ok(Err(reject("INVALID_ARGUMENT", "expected_cursor")));
        }
        if action == Action::DispatchOperation
            && definition
                .capabilities
                .contains(&Action::SetDispatchPriority)
        {
            for field in ["work_center", "expected_cursor"] {
                if !parameters.contains_key(field) {
                    return Ok(Err(reject("INVALID_ARGUMENT", field)));
                }
            }
        }
        if matches!(action, Action::CreateWorkOrder | Action::UpdateDraftOrder) {
            let id = parameters["work_order"].as_str().unwrap();
            let demand = parameters["source_demand"].as_str().unwrap();
            if !identifier(id) || demand.trim().is_empty() {
                return Ok(Err(reject(
                    "INVALID_ARGUMENT",
                    if !identifier(id) {
                        "work_order"
                    } else {
                        "source_demand"
                    },
                )));
            }
            let key = parameters["idempotency_key"].as_str().unwrap();
            let request = json!({"action":action,"arguments":parameters}).to_string();
            let saved: Option<(String, String)> = tx
                .query_row(
                    "SELECT request,response FROM scenario_requests WHERE run=?1 AND key=?2",
                    params![run, key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((original, response)) = saved {
                return Ok(if original == request {
                    Ok(serde_json::from_str(&response)?)
                } else {
                    Err(reject("IDEMPOTENCY_CONFLICT", "idempotency_key"))
                });
            }
            if parameters["expected_cursor"].as_i64() != Some(cursor) {
                return Ok(Err(reject("VERSION_CONFLICT", "expected_cursor")));
            }
            let template = if action == Action::CreateWorkOrder {
                if definition.work_orders.iter().any(|order| order == id) {
                    return Ok(Err(reject("WORK_ORDER_EXISTS", "work_order")));
                }
                let template = parameters["template_work_order"].as_str().unwrap();
                if created.contains_key(template)
                    || !definition.work_orders.iter().any(|order| order == template)
                {
                    return Ok(Err(reject("TEMPLATE_NOT_FOUND", "template_work_order")));
                }
                let serial = parameters.get("serial").and_then(Value::as_str);
                if let Some(setup) = &mut definition.assembly {
                    let Some(serial) = serial.filter(|value| identifier(value)) else {
                        return Ok(Err(reject("INVALID_ARGUMENT", "serial")));
                    };
                    if let Err(code) = setup.add_order(id, template, serial) {
                        return Ok(Err(reject(code, "serial")));
                    }
                } else if serial.is_some() {
                    return Ok(Err(reject("INVALID_ARGUMENT", "serial")));
                }
                tx.execute(
                    "INSERT INTO scenario_created_orders (run,id,template,demand,serial) VALUES (?1,?2,?3,?4,?5)",
                    params![run,id,template,demand,serial],
                )?;
                template
            } else {
                let Some((template, _)) = created.get(id) else {
                    return Ok(Err(reject("WORK_ORDER_NOT_FOUND", "work_order")));
                };
                if !read_work_order(&tx, run, &definition, id)?
                    .is_some_and(|order| order["status"] == "planned")
                {
                    return Ok(Err(reject("INVALID_STATE", "work_order")));
                }
                tx.execute(
                    "UPDATE scenario_created_orders SET demand=?3 WHERE run=?1 AND id=?2",
                    params![run, id, demand],
                )?;
                template
            };
            let response = json!({"ok":true,"run_id":run,"work_order":id,"status":"planned",
                "source_demand":demand,"template_work_order":template,"cursor":cursor+1});
            tx.execute(
                "UPDATE scenario_runs SET cursor=cursor+1 WHERE id=?1",
                [run],
            )?;
            tx.execute(
                "INSERT INTO scenario_requests VALUES (?1,?2,?3,?4)",
                params![run, key, request, response.to_string()],
            )?;
            tx.commit()?;
            return Ok(Ok(response));
        }
        if action == Action::ReleaseWorkOrders {
            let key = parameters["idempotency_key"].as_str().unwrap();
            let request = json!({"action":action,"arguments":parameters}).to_string();
            let saved: Option<(String, String)> = tx
                .query_row(
                    "SELECT request,response FROM scenario_requests WHERE run=?1 AND key=?2",
                    params![run, key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((original, response)) = saved {
                return Ok(if original == request {
                    Ok(serde_json::from_str(&response)?)
                } else {
                    Err(reject("IDEMPOTENCY_CONFLICT", "idempotency_key"))
                });
            }
            if parameters["expected_cursor"].as_i64() != Some(cursor) {
                return Ok(Err(reject("VERSION_CONFLICT", "expected_cursor")));
            }
            let mut apply_item: Box<
                dyn FnMut(&Map<String, Value>, i64) -> anyhow::Result<(Value, bool)>,
            > = if let Some(setup) = &definition.assembly {
                let model = simulation::load_model(
                    &definition.model,
                    definition.model_version,
                    &definition.model_parameters,
                )
                .map_err(anyhow::Error::msg)?;
                let mut simulation = stored.unwrap_or_else(|| {
                    Simulation::new(
                        seed.parse().expect("stored seed"),
                        &definition.event_rules,
                        model.as_ref(),
                    )
                });
                let tx = &tx;
                let rules = &definition.event_rules;
                Box::new(move |args, version| {
                    crate::assembly::invoke(
                        tx,
                        run,
                        setup,
                        Action::ReleaseWorkOrder,
                        args,
                        version,
                        true,
                        &mut simulation,
                        rules,
                        model.as_ref(),
                    )
                })
            } else if let Some(setup) = &definition.manufacturing {
                Box::new(|args, version| {
                    crate::mes::invoke(
                        &tx,
                        run,
                        setup,
                        &definition.work_orders,
                        Action::ReleaseWorkOrder,
                        args,
                        version,
                    )
                })
            } else {
                return Ok(Err(reject("TOOL_UNAVAILABLE", "name")));
            };
            let mut current = cursor;
            let mut results = Vec::new();
            let mut seen = std::collections::BTreeSet::new();
            // The prefix exceeds the public key length, so no direct request can claim an item key.
            let prefix = "_".repeat(257);
            for (index, id) in parameters["work_orders"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                let mut result = if seen.insert(id.as_str().unwrap()) {
                    let args = json!({"work_order":id,"expected_cursor":current,
                        "idempotency_key":format!("{prefix}{index}:{key}")});
                    let (response, changed) = apply_item(args.as_object().unwrap(), current)?;
                    if changed {
                        current += 1;
                    }
                    response
                } else {
                    json!({"ok":false,"code":"DUPLICATE_WORK_ORDER"})
                };
                result["work_order"] = id.clone();
                results.push(result);
            }
            drop(apply_item);
            let response = json!({"ok":true,"run_id":run,"cursor":current,"results":results});
            tx.execute(
                "INSERT INTO scenario_requests VALUES (?1,?2,?3,?4)",
                params![run, key, request, response.to_string()],
            )?;
            tx.commit()?;
            return Ok(Ok(response));
        }
        if action == Action::GetReleaseReadiness
            && let Some(setup) = &definition.assembly
        {
            let mut response = crate::assembly::release_readiness(
                &tx,
                run,
                setup,
                parameters["work_order"].as_str().unwrap(),
                cursor,
            )?;
            if response["ok"] == true
                && let Some(snapshot) = definition_snapshot
            {
                response["definition_snapshot"] = snapshot;
            }
            return Ok(Ok(response));
        }
        if crate::fabrication::tools().any(|tool| tool.0 == action) {
            let Some(setup) = definition
                .fabrication
                .filter(|setup| setup.valid(&definition.model, action))
            else {
                return Ok(Err(reject("TOOL_UNAVAILABLE", "name")));
            };
            let events = stored
                .as_ref()
                .map_or(&[][..], |state| state.timeline.events.as_slice());
            let (response, changed) =
                crate::fabrication::invoke(&tx, run, setup, action, parameters, cursor, events)?;
            if changed {
                tx.commit()?;
            }
            return Ok(Ok(response));
        }
        if crate::assembly::TOOLS.iter().any(|tool| tool.0 == action)
            || (matches!(
                action,
                Action::ReleaseWorkOrder
                    | Action::HoldWorkOrder
                    | Action::ResumeWorkOrder
                    | Action::CancelWorkOrder
                    | Action::CompleteWorkOrder
                    | Action::CloseWorkOrder
            ) && definition.assembly.is_some())
            || (definition.assembly.is_some()
                && matches!(
                    action,
                    Action::DispatchOperation
                        | Action::ReassignOperation
                        | Action::StartOperation
                        | Action::PauseOperation
                        | Action::ResumeOperation
                        | Action::PostOperationReport
                        | Action::ListOperationReports
                        | Action::ReverseOperationReport
                        | Action::CompleteOperation
                        | Action::SetDispatchPriority
                ))
        {
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
            let (mut response, changed) = crate::assembly::invoke(
                &tx,
                run,
                setup,
                action,
                parameters,
                cursor,
                definition.capabilities.contains(&Action::ReleaseWorkOrder),
                &mut simulation,
                &definition.event_rules,
                model.as_ref(),
            )?;
            if changed {
                if action == Action::ReleaseWorkOrder
                    && let Some(snapshot) = &definition_snapshot
                {
                    crate::snapshots::freeze(&tx, run, parameters, snapshot, &mut response)?;
                }
                tx.commit()?;
            }
            return Ok(Ok(response));
        }
        if let Some(setup) = shared_smt {
            if action.is_genealogy_action() {
                if let Some(order) = parameters.get("work_order").and_then(Value::as_str)
                    && !definition.work_orders.iter().any(|id| id == order)
                {
                    return Ok(Err(reject("WORK_ORDER_NOT_FOUND", "work_order")));
                }
                return Ok(Ok(crate::smt::genealogy(
                    &tx, run, setup, action, parameters,
                )?));
            }
            if action == Action::GetReleaseReadiness {
                let order = parameters["work_order"].as_str().unwrap();
                let snapshot = definition_snapshot.as_ref().expect("configured snapshot");
                let route_ok = crate::smt::valid_frozen_placement(snapshot);
                let planned = crate::smt::work_order_status(&tx, run, setup, order)? == "planned";
                return Ok(Ok(
                    json!({"ok":true,"work_order":order,"ready":route_ok && planned,
                    "blocking":if !planned {Some("INVALID_STATE")} else if !route_ok {Some("CELL_NOT_CAPABLE")} else {None},
                    "cursor":cursor,"definition_snapshot":snapshot}),
                ));
            }
        }
        if crate::smt::TOOLS.iter().any(|tool| tool.0 == action)
            || (shared_smt.is_some() && action == Action::ReleaseWorkOrder)
        {
            let Some(setup) = &definition.smt else {
                return Ok(Err(reject("TOOL_UNAVAILABLE", "name")));
            };
            let events = stored
                .as_ref()
                .map_or(&[][..], |state| state.timeline.events.as_slice());
            if action == Action::ReleaseWorkOrder
                && !definition_snapshot
                    .as_ref()
                    .is_some_and(crate::smt::valid_frozen_placement)
            {
                return Ok(Ok(json!({"ok":false,"code":"CELL_NOT_CAPABLE"})));
            }
            let (mut response, changed) =
                crate::smt::invoke(&tx, run, setup, action, parameters, cursor, events)?;
            if changed {
                if let Some(snapshot) = definition_snapshot.as_ref() {
                    crate::snapshots::freeze(&tx, run, parameters, snapshot, &mut response)?;
                }
                tx.commit()?;
            }
            return Ok(Ok(response));
        }
        if action.is_genealogy_action()
            || matches!(
                action,
                Action::GetWip
                    | Action::MoveWip
                    | Action::ReverseWipMovement
                    | Action::ReceiveWorkOrderOutput
                    | Action::ReconcileWorkOrder
                    | Action::SplitWip
                    | Action::MergeWip
                    | Action::ConsolidateWip
            )
            || crate::mes::TOOLS.iter().any(|tool| tool.0 == action)
        {
            let Some(setup) = &definition.manufacturing else {
                return Ok(Err(reject("TOOL_UNAVAILABLE", "name")));
            };
            let (mut response, changed) = crate::mes::invoke(
                &tx,
                run,
                setup,
                &definition.work_orders,
                action,
                parameters,
                cursor,
            )?;
            if action == Action::MesStatus
                && definition
                    .capabilities
                    .contains(&Action::PostOperationReport)
                && response["ok"] == true
            {
                if let Some(actions) = response["available_actions"].as_array_mut() {
                    for action in actions.iter_mut() {
                        if *action == "report_operation" {
                            *action = json!("post_operation_report");
                        }
                        if *action == "reverse_report" {
                            *action = json!("reverse_operation_report");
                        }
                    }
                }
            }
            if action == Action::MesStatus
                && definition.capabilities.contains(&Action::CloseWorkOrder)
                && response["available_actions"] == json!(["close_order"])
            {
                response["available_actions"] = if response["state"]["completion_confirmed"] == true
                {
                    json!(["close_work_order"])
                } else {
                    json!(["complete_work_order"])
                };
            }
            if action == Action::GetReleaseReadiness
                && response["ok"] == true
                && let Some(snapshot) = &definition_snapshot
            {
                response["definition_snapshot"] = snapshot.clone();
            }
            if changed {
                if action == Action::ReleaseWorkOrder
                    && let Some(snapshot) = &definition_snapshot
                {
                    crate::snapshots::freeze(&tx, run, parameters, snapshot, &mut response)?;
                }
                tx.commit()?;
            }
            return Ok(Ok(response));
        }
        if matches!(
            action,
            Action::ListOperations | Action::GetOperation | Action::ListDispatchQueue
        ) {
            let filter = parameters.get("work_order").and_then(Value::as_str);
            if filter.is_some_and(|id| !definition.work_orders.iter().any(|order| order == id)) {
                return Ok(Err(reject("WORK_ORDER_NOT_FOUND", "work_order")));
            }
            let mut rows = if let Some(setup) = &definition.assembly {
                crate::assembly::operations(&tx, run, setup, filter)?
            } else if let Some(setup) = &definition.manufacturing {
                crate::mes::operations(&tx, run, setup, &definition.work_orders, filter)?
            } else {
                return Ok(Err(reject("TOOL_UNAVAILABLE", "name")));
            };
            if action == Action::GetOperation {
                return Ok(rows
                    .into_iter()
                    .find(|row| Some(row.operation) == parameters["operation"].as_str())
                    .map(|row| json!({"ok":true,"run_id":run,"operation":row}))
                    .ok_or_else(|| reject("OPERATION_NOT_FOUND", "operation")));
            }
            if action == Action::ListDispatchQueue {
                let center = parameters.get("work_center").and_then(Value::as_str);
                rows.retain(|row| {
                    row.status == "ready"
                        && center.is_none_or(|center| row.work_center.as_deref() == Some(center))
                });
                rows.sort_by(|a, b| {
                    (
                        std::cmp::Reverse(a.priority),
                        a.due,
                        &a.work_order,
                        a.sequence,
                    )
                        .cmp(&(
                            std::cmp::Reverse(b.priority),
                            b.due,
                            &b.work_order,
                            b.sequence,
                        ))
                });
            }
            return Ok(Ok(json!({"ok":true,"run_id":run,"operations":rows})));
        }
        if action == Action::GetWorkOrder {
            let order = parameters["work_order"].as_str().unwrap();
            let Some(mut detail) = read_work_order(&tx, run, &definition, order)? else {
                return Ok(Err(reject("WORK_ORDER_NOT_FOUND", "work_order")));
            };
            if let Some((template, demand)) = created.get(order) {
                detail["source_demand"] = json!(demand);
                detail["template_work_order"] = json!(template);
            }
            return Ok(Ok(detail));
        }
        if action == Action::ListWorkOrders {
            let limit = parameters
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(50) as usize;
            let after = parameters.get("after_work_order").and_then(Value::as_str);
            let status = parameters.get("status").and_then(Value::as_str);
            let mut ids: Vec<_> = definition.work_orders.iter().map(String::as_str).collect();
            ids.sort_unstable();
            let mut orders = Vec::new();
            let mut next = None;
            for id in ids
                .into_iter()
                .filter(|id| after.is_none_or(|after| *id > after))
            {
                let Some(order) = read_work_order(&tx, run, &definition, id)? else {
                    continue;
                };
                if status.is_some_and(|status| order["status"] != status) {
                    continue;
                }
                if orders.len() == limit {
                    next = orders
                        .last()
                        .and_then(|order: &Value| order["work_order"].as_str());
                    break;
                }
                orders.push(json!({"work_order":id,"status":order["status"]}));
            }
            return Ok(Ok(
                json!({"ok":true,"run_id":run,"orders":orders,"next_after_work_order":next}),
            ));
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
    closed: bool,
    cursor: i64,
    state: Option<&Simulation>,
) -> Value {
    json!({"ok":true,"run_id":run,"scenario_id":scenario,"version":version,"closed":closed,"clock":state.map_or(0, |state| state.timeline.clock),"event_cursor":state.map_or(0, |state| state.timeline.events.len()),"cursor":cursor})
}
