use rusqlite::{OptionalExtension, Row, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

use crate::{
    catalog::{Action, object, text},
    scenario::{Rejection, identifier, reject},
};

const MAX_MEASUREMENTS: usize = 100;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Characteristic {
    unit: String,
    lower_limit: f64,
    upper_limit: f64,
}

#[derive(Serialize)]
struct InspectionPlan {
    plan_id: String,
    revision: u32,
    specification_version: String,
    characteristics: BTreeMap<String, Characteristic>,
    status: String,
}

pub(crate) fn characteristics_schema() -> Value {
    json!({"type":"object","minProperties":1,"maxProperties":MAX_MEASUREMENTS,
        "propertyNames":{"pattern":"^[A-Za-z0-9_-]{1,64}$"},
        "additionalProperties":object(&["unit","lower_limit","upper_limit"],
            json!({"unit":text(),"lower_limit":{"type":"number"},"upper_limit":{"type":"number"}}))})
}

pub(crate) fn plan_schema() -> Value {
    object(
        &[
            "plan_id",
            "revision",
            "specification_version",
            "characteristics",
            "status",
        ],
        json!({"plan_id":text(),"revision":{"type":"integer","minimum":1,"maximum":u32::MAX},
            "specification_version":text(),"characteristics":characteristics_schema(),
            "status":{"enum":["draft","active","superseded"]}}),
    )
}

fn inspection_plan(
    tx: &Transaction<'_>,
    run: &str,
    specification: &str,
) -> anyhow::Result<Option<InspectionPlan>> {
    let row = tx
        .query_row(
            "SELECT plan,revision,characteristics,status FROM quality_inspection_plans
         WHERE run=?1 AND specification=?2",
            params![run, specification],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get::<_, String>(2)?,
                    row.get(3)?,
                ))
            },
        )
        .optional()?;
    row.map(|(plan_id, revision, characteristics, status)| {
        Ok(InspectionPlan {
            plan_id,
            revision,
            specification_version: specification.into(),
            characteristics: serde_json::from_str(&characteristics)?,
            status,
        })
    })
    .transpose()
}

fn write_plan(
    tx: &Transaction<'_>,
    run: &str,
    action: Action,
    args: &Map<String, Value>,
) -> anyhow::Result<Result<Value, Rejection>> {
    let plan_id = args["plan_id"].as_str().unwrap();
    let revision = args["revision"].as_i64().unwrap();
    let specification = format!("{plan_id}@{revision}");
    if action == Action::CreateInspectionPlanRevision {
        let Ok(characteristics) = serde_json::from_value::<BTreeMap<String, Characteristic>>(
            args["characteristics"].clone(),
        ) else {
            return Ok(Err(Rejection::invalid("characteristics")));
        };
        if characteristics.is_empty()
            || characteristics.len() > MAX_MEASUREMENTS
            || characteristics.iter().any(|(name, item)| {
                !identifier(name)
                    || item.unit.is_empty()
                    || item.unit.trim() != item.unit
                    || item.unit.chars().count() > 256
                    || !item.lower_limit.is_finite()
                    || !item.upper_limit.is_finite()
                    || item.lower_limit > item.upper_limit
            })
        {
            return Ok(Err(Rejection::invalid("characteristics")));
        }
        let inserted = tx.execute(
            "INSERT INTO quality_inspection_plans (run,plan,revision,specification,characteristics,status)
             VALUES (?1,?2,?3,?4,?5,'draft') ON CONFLICT(run,plan,revision) DO NOTHING",
            params![run,plan_id,revision,specification,serde_json::to_string(&characteristics)?],
        )?;
        if inserted == 0 {
            return Ok(Err(reject("PLAN_REVISION_ALREADY_EXISTS", "revision")));
        }
    } else {
        let Some(plan) = inspection_plan(tx, run, &specification)? else {
            return Ok(Err(reject("PLAN_REVISION_NOT_FOUND", "revision")));
        };
        let active: Option<i64> = tx.query_row(
            "SELECT revision FROM quality_inspection_plans WHERE run=?1 AND plan=?2 AND status='active'",
            params![run,plan_id], |row| row.get(0),
        ).optional()?;
        if plan.status != "draft" || active.is_some_and(|current| current >= revision) {
            return Ok(Err(reject("PLAN_REVISION_NOT_ACTIVATABLE", "revision")));
        }
        tx.execute(
            "UPDATE quality_inspection_plans SET status='superseded' WHERE run=?1 AND plan=?2 AND status='active'",
            params![run,plan_id],
        )?;
        tx.execute(
            "UPDATE quality_inspection_plans SET status='active' WHERE run=?1 AND plan=?2 AND revision=?3",
            params![run,plan_id,revision],
        )?;
    }
    Ok(Ok(json!({"plan":inspection_plan(tx,run,&specification)?})))
}

#[derive(Serialize)]
struct Inspection {
    inspection_id: String,
    work_order: String,
    specification_version: String,
    status: String,
}

impl Inspection {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            inspection_id: row.get(0)?,
            work_order: row.get(1)?,
            specification_version: row.get(2)?,
            status: row.get(3)?,
        })
    }
}

#[derive(Serialize)]
struct Measurement {
    characteristic: String,
    value: f64,
    unit: String,
    lower_limit: f64,
    upper_limit: f64,
    passed: bool,
}

pub(crate) fn measurement_schema() -> Value {
    object(
        &[
            "characteristic",
            "value",
            "unit",
            "lower_limit",
            "upper_limit",
            "passed",
        ],
        json!({"characteristic":text(),"value":{"type":"number"},"unit":text(),
            "lower_limit":{"type":"number"},"upper_limit":{"type":"number"},"passed":{"type":"boolean"}}),
    )
}

pub(crate) fn nonconformance_schema() -> Value {
    let entry = json!({"type":"object","properties":{
        "action":{"enum":["open_nonconformance","place_quality_hold","disposition_nonconformance","release_quality_hold"]},
        "responsible":text(),"reason":text(),"evidence":text(),
        "decision":{"enum":["reinspection","concession","scrap"]},"reinspection_id":text(),
        "cursor":{"type":"integer","minimum":1}},
        "required":["action","responsible","reason","evidence","cursor"],"additionalProperties":false});
    object(
        &["inspection_id", "work_order", "status", "history"],
        json!({
        "inspection_id":text(),"work_order":text(),"status":{"enum":["open","held","disposed","released"]},
        "history":{"type":"array","items":entry}}),
    )
}

fn receiving_target(tx: &Transaction<'_>, run: &str, id: &str) -> rusqlite::Result<Option<String>> {
    tx.query_row(
        "SELECT receipt FROM stock_receipt_inspections WHERE run=?1 AND inspection=?2",
        params![run, id],
        |row| row.get(0),
    )
    .optional()
}

fn nonconformance(tx: &Transaction<'_>, run: &str, id: &str) -> anyhow::Result<Option<Value>> {
    let state: Option<String> = tx
        .query_row(
            "SELECT state FROM quality_nonconformances WHERE run=?1 AND inspection=?2",
            params![run, id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(state.as_deref().map(serde_json::from_str).transpose()?)
}

pub(crate) fn blocking(
    tx: &Transaction<'_>,
    run: &str,
    order: &str,
) -> rusqlite::Result<Option<String>> {
    tx.query_row(
        "SELECT CASE WHEN json_extract(n.state,'$.history[2].decision') = 'scrap'
                      AND json_extract(n.state,'$.status') = 'released'
                     THEN 'QUALITY_SCRAPPED' ELSE 'QUALITY_HOLD' END AS code
         FROM quality_inspections i
         JOIN quality_inspection_results r ON r.run=i.run AND r.inspection=i.id
         LEFT JOIN quality_nonconformances n ON n.run=i.run AND n.inspection=i.id
         WHERE i.run=?1 AND i.work_order=?2 AND r.status='failed'
         AND NOT EXISTS(SELECT 1 FROM stock_receipt_inspections a WHERE a.run=i.run AND a.inspection=i.id)
         AND (
             coalesce(json_extract(n.state,'$.status'),'open') <> 'released'
             OR json_extract(n.state,'$.history[2].decision') = 'scrap')
         ORDER BY code DESC LIMIT 1",
        params![run, order],
        |row| row.get(0),
    )
    .optional()
}

fn transition(
    tx: &Transaction<'_>,
    run: &str,
    inspection: &Inspection,
    action: Action,
    args: &Map<String, Value>,
    cursor: i64,
) -> anyhow::Result<Result<Value, Rejection>> {
    if inspection.status != "failed" {
        return Ok(Err(reject("FAILED_INSPECTION_REQUIRED", "inspection_id")));
    }
    let id = &inspection.inspection_id;
    let saved = nonconformance(tx, run, id)?;
    let state = saved.as_ref().and_then(|row| row["status"].as_str());
    let status = match (action, state) {
        (Action::OpenNonconformance, None) => "open",
        (Action::PlaceQualityHold, Some("open")) => "held",
        (Action::DispositionNonconformance, Some("held")) => "disposed",
        (Action::ReleaseQualityHold, Some("disposed")) => "released",
        _ => return Ok(Err(reject("NONCONFORMANCE_STATE_INVALID", "inspection_id"))),
    };
    for field in ["responsible", "reason", "evidence"] {
        if args[field].as_str().unwrap().trim().is_empty() {
            return Ok(Err(Rejection::invalid(field)));
        }
    }
    let mut entry = json!({"action":action,"responsible":args["responsible"],
        "reason":args["reason"],"evidence":args["evidence"],"cursor":cursor+1});
    if action == Action::DispositionNonconformance {
        entry["decision"] = args["decision"].clone();
        if args["decision"] == "reinspection" {
            let Some(reinspection) = args["reinspection_id"].as_str() else {
                return Ok(Err(Rejection::invalid("reinspection_id")));
            };
            let acceptable: bool = tx.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM quality_inspections i
                    JOIN quality_inspection_results r ON r.run=i.run AND r.inspection=i.id
                    JOIN quality_inspection_results failed ON failed.run=i.run AND failed.inspection=?3
                    WHERE i.run=?1 AND i.id=?2 AND r.status='passed' AND r.rowid>failed.rowid
                    AND i.work_order=?4 AND i.specification_version=?5
                    AND (SELECT receipt FROM stock_receipt_inspections WHERE run=?1 AND inspection=?2)
                        IS (SELECT receipt FROM stock_receipt_inspections WHERE run=?1 AND inspection=?3)
                    AND NOT EXISTS (
                        SELECT 1 FROM quality_measurements original
                        WHERE original.run=?1 AND original.inspection=?3 AND NOT original.passed
                        AND NOT EXISTS (
                            SELECT 1 FROM quality_measurements repeat
                            WHERE repeat.run=?1 AND repeat.inspection=?2
                            AND repeat.characteristic=original.characteristic
                            AND repeat.unit=original.unit AND repeat.lower_limit=original.lower_limit
                            AND repeat.upper_limit=original.upper_limit AND repeat.passed
                        )
                    )
                 )",
                params![run, reinspection, id, inspection.work_order, inspection.specification_version],
                |row| row.get(0),
            )?;
            if !acceptable {
                return Ok(Err(reject(
                    "REINSPECTION_NOT_ACCEPTABLE",
                    "reinspection_id",
                )));
            }
            entry["reinspection_id"] = json!(reinspection);
        } else if args.contains_key("reinspection_id") {
            return Ok(Err(Rejection::invalid("reinspection_id")));
        }
    }
    let mut item = saved.unwrap_or_else(|| {
        json!({
            "inspection_id":id,"work_order":inspection.work_order,"history":[]
        })
    });
    item["status"] = json!(status);
    item["history"].as_array_mut().unwrap().push(entry);
    tx.execute(
        "INSERT INTO quality_nonconformances (run,inspection,state) VALUES (?1,?2,?3)
         ON CONFLICT(run,inspection) DO UPDATE SET state=excluded.state",
        params![run, id, item.to_string()],
    )?;
    Ok(Ok(json!({"nonconformance":item})))
}

fn inspection(tx: &Transaction<'_>, run: &str, id: &str) -> rusqlite::Result<Option<Inspection>> {
    tx.query_row(
        "SELECT i.id,i.work_order,i.specification_version,coalesce(r.status,'pending')
         FROM quality_inspections i LEFT JOIN quality_inspection_results r
         ON r.run=i.run AND r.inspection=i.id WHERE i.run=?1 AND i.id=?2",
        params![run, id],
        Inspection::read,
    )
    .optional()
}

fn measurements(tx: &Transaction<'_>, run: &str, id: &str) -> rusqlite::Result<Vec<Measurement>> {
    let mut query = tx.prepare(
        "SELECT characteristic,value,unit,lower_limit,upper_limit,passed FROM quality_measurements
         WHERE run=?1 AND inspection=?2 ORDER BY characteristic",
    )?;
    query
        .query_map(params![run, id], |row| {
            Ok(Measurement {
                characteristic: row.get(0)?,
                value: row.get(1)?,
                unit: row.get(2)?,
                lower_limit: row.get(3)?,
                upper_limit: row.get(4)?,
                passed: row.get(5)?,
            })
        })?
        .collect()
}

pub(crate) fn add_machining_inspection_schema(action: Action, spec: &mut crate::catalog::Spec) {
    if action == Action::CreateInspection {
        spec.input["properties"]["report_id"] = crate::inventory::id_schema();
        spec.errors = &[
            "INVALID_ARGUMENT",
            "WORK_ORDER_NOT_FOUND",
            "INSPECTION_ALREADY_EXISTS",
            "PLAN_REVISION_NOT_FOUND",
            "PLAN_REVISION_NOT_ACTIVE",
            "INSPECTION_PLAN_SNAPSHOT_MISMATCH",
            "RECEIPT_INSPECTION_NOT_PENDING",
            "REINSPECTION_NOT_ACCEPTABLE",
            "MACHINING_REPORT_NOT_FOUND",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
        ];
    }
    spec.output["oneOf"][0]["properties"]["machining_inspection"] = crate::catalog::object(
        &["report_id", "kind"],
        json!({"report_id":crate::inventory::id_schema(),"kind":{"enum":["first_article","in_process"]}}),
    );
}

fn machining_target(
    tx: &Transaction<'_>,
    run: &str,
    id: &str,
) -> rusqlite::Result<Option<(String, String)>> {
    tx.query_row(
        "SELECT report,kind FROM quality_machining_inspections WHERE run=?1 AND inspection=?2",
        params![run, id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

pub(crate) fn machining_passed(
    tx: &Transaction<'_>,
    run: &str,
    report: &str,
) -> rusqlite::Result<bool> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM quality_machining_inspections m
        JOIN quality_inspection_results r ON r.run=m.run AND r.inspection=m.inspection
        WHERE m.run=?1 AND m.report=?2 AND r.status='passed')",
        params![run, report],
        |row| row.get(0),
    )
}

pub(crate) fn invoke(
    tx: Transaction<'_>,
    run: &str,
    work_orders: &[String],
    action: Action,
    arguments: &Map<String, Value>,
    cursor: i64,
    versioned_plans: bool,
) -> anyhow::Result<Result<Value, Rejection>> {
    for field in [
        "inspection_id",
        "after_inspection_id",
        "reinspection_id",
        "reinspection_of",
        "plan_id",
    ] {
        if arguments
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|id| !identifier(id))
        {
            return Ok(Err(Rejection::invalid(field)));
        }
    }
    let work_order = arguments.get("work_order").and_then(Value::as_str);
    if work_order.is_some_and(|order| !work_orders.iter().any(|id| id == order)) {
        return Ok(Err(reject("WORK_ORDER_NOT_FOUND", "work_order")));
    }
    if action == Action::ListInspections {
        let after = arguments
            .get("after_inspection_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let limit = arguments.get("limit").and_then(Value::as_i64).unwrap_or(20);
        let mut query = tx.prepare(
            "SELECT i.id,i.work_order,i.specification_version,coalesce(r.status,'pending')
             FROM quality_inspections i LEFT JOIN quality_inspection_results r
             ON r.run=i.run AND r.inspection=i.id
             WHERE i.run=?1 AND i.id>?2 AND (?3 IS NULL OR i.work_order=?3)
             ORDER BY i.id LIMIT ?4",
        )?;
        let mut inspections = query
            .query_map(params![run, after, work_order, limit + 1], Inspection::read)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next = if inspections.len() > limit as usize {
            inspections.pop();
            inspections.last().map(|row| row.inspection_id.as_str())
        } else {
            None
        };
        return Ok(Ok(json!({"ok":true,"run_id":run,"inspections":inspections,
            "next_after_inspection_id":next})));
    }

    let id = arguments
        .get("inspection_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if action == Action::GetInspection {
        let Some(inspection) = inspection(&tx, run, id)? else {
            return Ok(Err(reject("INSPECTION_NOT_FOUND", "inspection_id")));
        };
        let measurements = measurements(&tx, run, id)?;
        let plan = inspection_plan(&tx, run, &inspection.specification_version)?;
        let mut response = json!({"ok":true,"run_id":run,"inspection":inspection});
        if let Some(receipt) = receiving_target(&tx, run, id)? {
            response["receipt_id"] = json!(receipt);
        }
        if let Some((report, kind)) = machining_target(&tx, run, id)? {
            response["machining_inspection"] = json!({"report_id":report,"kind":kind});
        }
        if let Some(plan) = plan {
            response["inspection_plan"] = json!(plan);
        }
        if !measurements.is_empty() {
            response["measurements"] = json!(measurements);
        }
        if let Some(item) = nonconformance(&tx, run, id)? {
            response["nonconformance"] = item;
        }
        return Ok(Ok(response));
    }

    let key = arguments["idempotency_key"].as_str().unwrap();
    let request = json!({"action":action,"arguments":arguments}).to_string();
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
    if arguments["expected_cursor"].as_i64() != Some(cursor) {
        return Ok(Err(reject("VERSION_CONFLICT", "expected_cursor")));
    }

    let mut response = if matches!(
        action,
        Action::CreateInspectionPlanRevision | Action::ActivateInspectionPlanRevision
    ) {
        match write_plan(&tx, run, action, arguments)? {
            Ok(response) => response,
            Err(error) => return Ok(Err(error)),
        }
    } else if action == Action::CreateInspection {
        let specification = arguments["specification_version"].as_str().unwrap();
        if specification.trim().is_empty() {
            return Ok(Err(Rejection::invalid("specification_version")));
        }
        let work_order = work_order.unwrap();
        let receipt = arguments.get("receipt_id").and_then(Value::as_str);
        if let Some(receipt) = receipt {
            if !identifier(receipt) {
                return Ok(Err(Rejection::invalid("receipt_id")));
            }
            if !crate::inventory::pending_receipt(&tx, run, receipt)? {
                return Ok(Err(reject("RECEIPT_INSPECTION_NOT_PENDING", "receipt_id")));
            }
        }
        let reinspection_of = arguments.get("reinspection_of").and_then(Value::as_str);
        let report = arguments.get("report_id").and_then(Value::as_str);
        if report.is_some_and(|report| !identifier(report))
            || (report.is_some() && receipt.is_some())
        {
            return Ok(Err(Rejection::invalid("report_id")));
        }
        let kind = if let Some(report) = report {
            let Some(kind) = crate::mes::machining_inspection_kind(&tx, run, work_order, report)?
            else {
                return Ok(Err(reject("MACHINING_REPORT_NOT_FOUND", "report_id")));
            };
            Some(kind)
        } else {
            None
        };
        if let Some(original) = reinspection_of {
            let prior = machining_target(&tx, run, original)?;
            if prior.as_ref().map(|(report, _)| report.as_str()) != report {
                return Ok(Err(reject(
                    "REINSPECTION_NOT_ACCEPTABLE",
                    "reinspection_of",
                )));
            }
        }
        if versioned_plans {
            let Some(plan) = inspection_plan(&tx, run, specification)? else {
                return Ok(Err(reject(
                    "PLAN_REVISION_NOT_FOUND",
                    "specification_version",
                )));
            };
            let frozen = if receipt.is_none() {
                crate::snapshots::load(&tx, run, work_order)?
            } else {
                None
            };
            if frozen.as_ref().is_some_and(|snapshot| {
                snapshot["inspection_plan"]["specification_version"] != specification
            }) {
                return Ok(Err(reject(
                    "INSPECTION_PLAN_SNAPSHOT_MISMATCH",
                    "specification_version",
                )));
            }
            if let Some(original) = reinspection_of {
                let valid = inspection(&tx, run, original)?.is_some_and(|previous| {
                    previous.status == "failed"
                        && previous.work_order == work_order
                        && previous.specification_version == specification
                });
                if !valid || receiving_target(&tx, run, original)?.as_deref() != receipt {
                    return Ok(Err(reject(
                        "REINSPECTION_NOT_ACCEPTABLE",
                        "reinspection_of",
                    )));
                }
            } else if frozen.is_none() && plan.status != "active" {
                return Ok(Err(reject(
                    "PLAN_REVISION_NOT_ACTIVE",
                    "specification_version",
                )));
            }
        } else if reinspection_of.is_some() {
            return Ok(Err(Rejection::invalid("reinspection_of")));
        }
        let inserted = tx.execute(
            "INSERT INTO quality_inspections (run,id,work_order,specification_version)
             VALUES (?1,?2,?3,?4) ON CONFLICT(run,id) DO NOTHING",
            params![run, id, work_order, specification],
        )?;
        if inserted == 0 {
            return Ok(Err(reject("INSPECTION_ALREADY_EXISTS", "inspection_id")));
        }
        if let Some(receipt) = receipt {
            tx.execute(
                "INSERT INTO stock_receipt_inspections (run,inspection,receipt) VALUES (?1,?2,?3)",
                params![run, id, receipt],
            )?;
        }
        let mut result = json!({"inspection":{"inspection_id":id,"work_order":work_order,
            "specification_version":specification,"status":"pending"}});
        if let Some((report, kind)) = report.zip(kind) {
            tx.execute("INSERT INTO quality_machining_inspections(run,inspection,report,kind) VALUES(?1,?2,?3,?4)",
                params![run,id,report,kind])?;
            result["machining_inspection"] = json!({"report_id":report,"kind":kind});
        }
        result
    } else {
        let Some(mut inspection) = inspection(&tx, run, id)? else {
            return Ok(Err(reject("INSPECTION_NOT_FOUND", "inspection_id")));
        };
        if matches!(
            action,
            Action::OpenNonconformance
                | Action::PlaceQualityHold
                | Action::DispositionNonconformance
                | Action::ReleaseQualityHold
        ) {
            match transition(&tx, run, &inspection, action, arguments, cursor)? {
                Ok(response) => response,
                Err(error) => return Ok(Err(error)),
            }
        } else {
            if let Some(receipt) = receiving_target(&tx, run, id)?
                && !crate::inventory::pending_receipt(&tx, run, &receipt)?
            {
                return Ok(Err(reject(
                    "RECEIPT_INSPECTION_NOT_PENDING",
                    "inspection_id",
                )));
            }
            if inspection.status != "pending" {
                return Ok(Err(reject("INSPECTION_COMPLETED", "inspection_id")));
            }
            let recorded = measurements(&tx, run, id)?;
            let plan = inspection_plan(&tx, run, &inspection.specification_version)?;
            match action {
                Action::RecordMeasurement => {
                    let characteristic = arguments["characteristic"].as_str().unwrap();
                    let unit = arguments["unit"].as_str().unwrap();
                    for (field, value) in [("characteristic", characteristic), ("unit", unit)] {
                        if value.is_empty() || value.trim() != value {
                            return Ok(Err(Rejection::invalid(field)));
                        }
                    }
                    let value = arguments["value"].as_f64().unwrap();
                    let lower_limit = arguments["lower_limit"].as_f64().unwrap();
                    let upper_limit = arguments["upper_limit"].as_f64().unwrap();
                    if lower_limit > upper_limit {
                        return Ok(Err(Rejection::invalid("upper_limit")));
                    }
                    if let Some(plan) = &plan {
                        let Some(expected) = plan.characteristics.get(characteristic) else {
                            return Ok(Err(reject("CHARACTERISTIC_NOT_IN_PLAN", "characteristic")));
                        };
                        for (field, matches) in [
                            ("unit", unit == expected.unit),
                            ("lower_limit", lower_limit == expected.lower_limit),
                            ("upper_limit", upper_limit == expected.upper_limit),
                        ] {
                            if !matches {
                                return Ok(Err(reject(
                                    "MEASUREMENT_SPECIFICATION_MISMATCH",
                                    field,
                                )));
                            }
                        }
                    }
                    if recorded
                        .iter()
                        .any(|item| item.characteristic == characteristic)
                    {
                        return Ok(Err(reject("MEASUREMENT_ALREADY_EXISTS", "characteristic")));
                    }
                    if recorded.len() >= MAX_MEASUREMENTS {
                        return Ok(Err(reject("MEASUREMENT_LIMIT_REACHED", "inspection_id")));
                    }
                    let passed = (lower_limit..=upper_limit).contains(&value);
                    tx.execute(
                        "INSERT INTO quality_measurements
                     (run,inspection,characteristic,value,unit,lower_limit,upper_limit,passed)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                        params![
                            run,
                            id,
                            characteristic,
                            value,
                            unit,
                            lower_limit,
                            upper_limit,
                            passed
                        ],
                    )?;
                    json!({"measurement":{"characteristic":characteristic,"value":value,"unit":unit,
                    "lower_limit":lower_limit,"upper_limit":upper_limit,"passed":passed}})
                }
                Action::CompleteInspection => {
                    if recorded.is_empty() {
                        return Ok(Err(reject("MEASUREMENT_REQUIRED", "inspection_id")));
                    }
                    if plan
                        .as_ref()
                        .is_some_and(|plan| plan.characteristics.len() != recorded.len())
                    {
                        return Ok(Err(reject("PLAN_MEASUREMENTS_INCOMPLETE", "inspection_id")));
                    }
                    inspection.status = if recorded.iter().all(|item| item.passed) {
                        "passed"
                    } else {
                        "failed"
                    }
                    .into();
                    tx.execute(
                    "INSERT INTO quality_inspection_results (run,inspection,status) VALUES (?1,?2,?3)",
                    params![run, id, inspection.status],
                )?;
                    json!({"inspection":inspection})
                }
                _ => unreachable!("inspection write action"),
            }
        }
    };
    response["ok"] = json!(true);
    response["run_id"] = json!(run);
    response["cursor"] = json!(cursor + 1);
    tx.execute(
        "UPDATE scenario_runs SET cursor=cursor+1 WHERE id=?1",
        [run],
    )?;
    tx.execute(
        "INSERT INTO scenario_requests VALUES (?1,?2,?3,?4)",
        params![run, key, request, response.to_string()],
    )?;
    tx.commit()?;
    Ok(Ok(response))
}
