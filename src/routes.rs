use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    bom::item_revision,
    catalog::{Action, Spec, object, text},
    scenario::{Rejection, identifier, reject},
};

const MAX_OPERATIONS: usize = 100;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    work_center: String,
    predecessors: Vec<String>,
}

#[derive(Serialize)]
struct Route {
    item_id: String,
    revision: i64,
    status: String,
    operations: BTreeMap<String, Operation>,
    execution_order: Vec<String>,
}

fn id_schema() -> Value {
    json!({"type":"string","minLength":1,"maxLength":64,"pattern":"^[A-Za-z0-9_-]+$"})
}

fn operations_schema() -> Value {
    json!({"type":"object","propertyNames":id_schema(),"maxProperties":MAX_OPERATIONS,
        "additionalProperties":object(&["work_center","predecessors"],json!({"work_center":id_schema(),
            "predecessors":{"type":"array","items":id_schema(),"uniqueItems":true,"maxItems":MAX_OPERATIONS}}))})
}

pub(crate) fn revision_schema() -> Value {
    object(
        &[
            "item_id",
            "revision",
            "status",
            "operations",
            "execution_order",
        ],
        json!({
        "item_id":id_schema(),"revision":{"type":"integer","minimum":1,"maximum":u32::MAX},
        "status":{"enum":["draft","active","superseded"]},"operations":operations_schema(),
        "execution_order":{"type":"array","items":id_schema()}}),
    )
}

pub(crate) fn spec(action: Action) -> Spec {
    let mut required = vec!["item_id", "revision"];
    let mut fields =
        json!({"item_id":id_schema(),"revision":{"type":"integer","minimum":1,"maximum":u32::MAX}});
    let write = action != Action::GetRouteRevision;
    let mut output =
        json!({"ok":{"const":true},"run_id":text(),"route_revision":revision_schema()});
    let mut output_required = vec!["ok", "run_id", "route_revision"];
    if write {
        required.extend(["expected_cursor", "idempotency_key"]);
        fields["expected_cursor"] = json!({"type":"integer","minimum":0,"maximum":i64::MAX});
        fields["idempotency_key"] = text();
        output["cursor"] = json!({"type":"integer","minimum":0});
        output_required.push("cursor");
    }
    let description = match action {
        Action::CreateRouteRevision => "创建工艺路线版本",
        Action::GetRouteRevision => "查询工艺路线及执行顺序",
        Action::SetRouteOperations => {
            required.push("operations");
            fields["operations"] = operations_schema();
            "整体替换草稿工艺路线工序"
        }
        _ => unreachable!("route catalog action"),
    };
    Spec {
        name: action.name(),
        description,
        domain: "production",
        effect: if write { "write" } else { "read" },
        input: object(&required, fields),
        output: json!({"oneOf":[object(&output_required,output),object(&["ok","code","field"],
            json!({"ok":{"const":false},"code":text(),"field":text()}))]}),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: if write { action.name() } else { "none" },
        errors: &[
            "INVALID_ARGUMENT",
            "ITEM_REVISION_NOT_FOUND",
            "ROUTE_REVISION_NOT_FOUND",
            "ROUTE_REVISION_ALREADY_EXISTS",
            "ROUTE_REVISION_IMMUTABLE",
            "ROUTE_PREDECESSOR_NOT_FOUND",
            "ROUTE_DUPLICATE_PREDECESSOR",
            "ROUTE_CYCLE",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

fn execution_order(operations: &BTreeMap<String, Operation>) -> Result<Vec<String>, Rejection> {
    let mut remaining = BTreeMap::new();
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut ready = BTreeSet::new();
    for (id, operation) in operations {
        if !identifier(id)
            || !identifier(&operation.work_center)
            || operation.predecessors.len() > MAX_OPERATIONS
        {
            return Err(Rejection::invalid("operations"));
        }
        let mut unique = BTreeSet::new();
        for predecessor in &operation.predecessors {
            if !operations.contains_key(predecessor) {
                return Err(reject("ROUTE_PREDECESSOR_NOT_FOUND", "operations"));
            }
            if !unique.insert(predecessor) {
                return Err(reject("ROUTE_DUPLICATE_PREDECESSOR", "operations"));
            }
            dependents.entry(predecessor).or_default().push(id);
        }
        remaining.insert(id.as_str(), unique.len());
        if unique.is_empty() {
            ready.insert(id.as_str());
        }
    }
    let mut order = Vec::with_capacity(operations.len());
    while let Some(id) = ready.pop_first() {
        order.push(id.to_owned());
        if let Some(next) = dependents.get(id) {
            for successor in next {
                let remaining = remaining.get_mut(successor).unwrap();
                *remaining -= 1;
                if *remaining == 0 {
                    ready.insert(successor);
                }
            }
        }
    }
    if order.len() != operations.len() {
        return Err(reject("ROUTE_CYCLE", "operations"));
    }
    Ok(order)
}

fn read(
    tx: &Transaction<'_>,
    run: &str,
    item: &str,
    revision: i64,
) -> anyhow::Result<Option<Route>> {
    let row = tx
        .query_row(
            "SELECT status,operations FROM master_routes WHERE run=?1 AND item=?2 AND revision=?3",
            params![run, item, revision],
            |row| Ok((row.get(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    row.map(|(status, serialized)| {
        let operations = serde_json::from_str(&serialized)?;
        let execution_order = execution_order(&operations)
            .map_err(|_| anyhow::anyhow!("stored route operations are invalid"))?;
        Ok(Route {
            item_id: item.into(),
            revision,
            status,
            operations,
            execution_order,
        })
    })
    .transpose()
}

fn check_publish(
    tx: &Transaction<'_>,
    run: &str,
    route: &Route,
) -> rusqlite::Result<Result<(), Rejection>> {
    if route.operations.is_empty() {
        return Ok(Err(reject("ROUTE_OPERATIONS_REQUIRED", "operations")));
    }
    if item_revision(tx, run, &route.item_id, route.revision)?
        .is_none_or(|(_, status)| status == "draft")
    {
        return Ok(Err(reject("ROUTE_ITEM_REVISION_NOT_PUBLISHED", "item_id")));
    }
    if route.status == "draft" {
        let superseded:bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM master_routes WHERE run=?1 AND item=?2 AND revision>=?3 AND status='active')",
            params![run,route.item_id,route.revision],|row| row.get(0),
        )?;
        if superseded {
            return Ok(Err(reject("ROUTE_REVISION_NOT_ACTIVATABLE", "revision")));
        }
    }
    Ok(Ok(()))
}

pub(crate) fn invoke(
    tx: Transaction<'_>,
    run: &str,
    action: Action,
    args: &Map<String, Value>,
    cursor: i64,
) -> anyhow::Result<Result<Value, Rejection>> {
    let item = args["item_id"].as_str().unwrap();
    if !identifier(item) {
        return Ok(Err(Rejection::invalid("item_id")));
    }
    let number = args["revision"].as_i64().unwrap();
    if matches!(action, Action::GetRouteRevision | Action::ValidateRevision) {
        let Some(route) = read(&tx, run, item, number)? else {
            return Ok(Err(reject("ROUTE_REVISION_NOT_FOUND", "revision")));
        };
        if action == Action::GetRouteRevision {
            return Ok(Ok(json!({"ok":true,"run_id":run,"route_revision":route})));
        }
        return Ok(
            check_publish(&tx, run, &route)?.map(|()| json!({"ok":true,"run_id":run,"valid":true}))
        );
    }

    let key = args["idempotency_key"].as_str().unwrap();
    let request = json!({"action":action,"arguments":args}).to_string();
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
    if args["expected_cursor"].as_i64() != Some(cursor) {
        return Ok(Err(reject("VERSION_CONFLICT", "expected_cursor")));
    }

    if action == Action::CreateRouteRevision {
        if item_revision(&tx, run, item, number)?.is_none() {
            return Ok(Err(reject("ITEM_REVISION_NOT_FOUND", "revision")));
        }
        if tx.execute(
            "INSERT INTO master_routes (run,item,revision,operations,status)
            VALUES (?1,?2,?3,'{}','draft') ON CONFLICT(run,item,revision) DO NOTHING",
            params![run, item, number],
        )? == 0
        {
            return Ok(Err(reject("ROUTE_REVISION_ALREADY_EXISTS", "revision")));
        }
    } else {
        let Some(route) = read(&tx, run, item, number)? else {
            return Ok(Err(reject("ROUTE_REVISION_NOT_FOUND", "revision")));
        };
        if route.status != "draft" {
            return Ok(Err(reject("ROUTE_REVISION_IMMUTABLE", "revision")));
        }
        if action == Action::SetRouteOperations {
            let Ok(operations) =
                serde_json::from_value::<BTreeMap<String, Operation>>(args["operations"].clone())
            else {
                return Ok(Err(Rejection::invalid("operations")));
            };
            if operations.len() > MAX_OPERATIONS {
                return Ok(Err(Rejection::invalid("operations")));
            }
            if let Err(error) = execution_order(&operations) {
                return Ok(Err(error));
            }
            tx.execute(
                "UPDATE master_routes SET operations=?4 WHERE run=?1 AND item=?2 AND revision=?3",
                params![run, item, number, serde_json::to_string(&operations)?],
            )?;
        } else {
            if let Err(error) = check_publish(&tx, run, &route)? {
                return Ok(Err(error));
            }
            tx.execute("UPDATE master_routes SET status='superseded' WHERE run=?1 AND item=?2 AND status='active'",
                params![run,item])?;
            tx.execute(
                "UPDATE master_routes SET status='active' WHERE run=?1 AND item=?2 AND revision=?3",
                params![run, item, number],
            )?;
        }
    }
    let response = json!({"ok":true,"run_id":run,"cursor":cursor+1,"route_revision":read(&tx,run,item,number)?});
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
