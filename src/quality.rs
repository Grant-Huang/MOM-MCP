use rusqlite::{OptionalExtension, Row, Transaction, params};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{
    catalog::Action,
    scenario::{Rejection, identifier, reject},
};

#[derive(Serialize)]
struct Inspection {
    inspection_id: String,
    work_order: String,
    specification_version: String,
    status: &'static str,
}

impl Inspection {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            inspection_id: row.get(0)?,
            work_order: row.get(1)?,
            specification_version: row.get(2)?,
            status: "pending",
        })
    }
}

pub(crate) fn invoke(
    tx: Transaction<'_>,
    run: &str,
    work_orders: &[String],
    action: Action,
    arguments: &Map<String, Value>,
    cursor: i64,
) -> anyhow::Result<Result<Value, Rejection>> {
    for field in ["inspection_id", "after_inspection_id"] {
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

    match action {
        Action::ListInspections => {
            let after = arguments
                .get("after_inspection_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let limit = arguments.get("limit").and_then(Value::as_i64).unwrap_or(20);
            let mut query = tx.prepare(
                "SELECT id,work_order,specification_version FROM quality_inspections
                 WHERE run=?1 AND id>?2 AND (?3 IS NULL OR work_order=?3)
                 ORDER BY id LIMIT ?4",
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
            Ok(Ok(json!({"ok":true,"run_id":run,"inspections":inspections,
                "next_after_inspection_id":next})))
        }
        Action::GetInspection => {
            let id = arguments["inspection_id"].as_str().unwrap();
            let inspection = tx.query_row(
                "SELECT id,work_order,specification_version FROM quality_inspections WHERE run=?1 AND id=?2",
                params![run, id],
                Inspection::read,
            ).optional()?;
            Ok(inspection
                .map(|row| json!({"ok":true,"run_id":run,"inspection":row}))
                .ok_or_else(|| reject("INSPECTION_NOT_FOUND", "inspection_id")))
        }
        Action::CreateInspection => {
            let id = arguments["inspection_id"].as_str().unwrap();
            let specification = arguments["specification_version"].as_str().unwrap();
            if specification.trim().is_empty() {
                return Ok(Err(Rejection::invalid("specification_version")));
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
            let work_order = work_order.unwrap();
            let inserted = tx.execute(
                "INSERT INTO quality_inspections (run,id,work_order,specification_version)
                 VALUES (?1,?2,?3,?4) ON CONFLICT(run,id) DO NOTHING",
                params![run, id, work_order, specification],
            )?;
            if inserted == 0 {
                return Ok(Err(reject("INSPECTION_ALREADY_EXISTS", "inspection_id")));
            }
            let response = json!({"ok":true,"run_id":run,"cursor":cursor+1,
                "inspection":{"inspection_id":id,"work_order":work_order,
                "specification_version":specification,"status":"pending"}});
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
        _ => unreachable!("inspection action"),
    }
}
