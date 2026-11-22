use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    catalog::{Action, Spec, object, text},
    scenario::{Rejection, identifier, reject},
};

const MAX_COMPONENTS: usize = 100;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Component {
    revision: u32,
    quantity: f64,
    unit: String,
    #[serde(default)]
    expand_bom: bool,
}

#[derive(Serialize)]
struct Bom {
    item_id: String,
    revision: i64,
    status: String,
    components: BTreeMap<String, Component>,
}

fn id_schema() -> Value {
    json!({"type":"string","minLength":1,"maxLength":64,"pattern":"^[A-Za-z0-9_-]+$"})
}

fn revision_number() -> Value {
    json!({"type":"integer","minimum":1,"maximum":u32::MAX})
}

fn component_schema() -> Value {
    json!({"type":"object","propertyNames":id_schema(),"maxProperties":MAX_COMPONENTS,
        "additionalProperties":object(&["revision","quantity","unit"],json!({
            "revision":revision_number(),"quantity":{"type":"number","exclusiveMinimum":0},
            "unit":text(),"expand_bom":{"type":"boolean"}}))})
}

pub(crate) fn revision_schema() -> Value {
    object(
        &["item_id", "revision", "status", "components"],
        json!({
        "item_id":id_schema(),"revision":revision_number(),
        "status":{"enum":["draft","active","superseded"]},"components":component_schema()}),
    )
}

pub(crate) fn validation_spec(allow_route: bool) -> Spec {
    let mut spec = spec(Action::ValidateRevision);
    if allow_route {
        spec.input["properties"]["revision_type"]["enum"] = json!(["bom", "route"]);
        spec.description = "校验主数据版本的发布条件";
        spec.errors = &[
            "INVALID_ARGUMENT",
            "BOM_REVISION_NOT_FOUND",
            "BOM_REVISION_NOT_ACTIVATABLE",
            "BOM_COMPONENTS_REQUIRED",
            "COMPONENT_REVISION_NOT_FOUND",
            "COMPONENT_UNIT_MISMATCH",
            "COMPONENT_BOM_NOT_FOUND",
            "BOM_DEPENDENCY_NOT_PUBLISHED",
            "ROUTE_REVISION_NOT_FOUND",
            "ROUTE_REVISION_NOT_ACTIVATABLE",
            "ROUTE_OPERATIONS_REQUIRED",
            "ROUTE_ITEM_REVISION_NOT_PUBLISHED",
            "ROUTE_PREDECESSOR_NOT_FOUND",
            "ROUTE_DUPLICATE_PREDECESSOR",
            "ROUTE_CYCLE",
        ];
    }
    spec
}

pub(crate) fn spec(action: Action) -> Spec {
    let mut required = vec!["item_id", "revision"];
    let mut fields = json!({"item_id":id_schema(),"revision":revision_number()});
    let write = matches!(action, Action::CreateBomRevision | Action::SetBomComponents);
    if write {
        required.extend(["expected_cursor", "idempotency_key"]);
        fields["expected_cursor"] = json!({"type":"integer","minimum":0,"maximum":i64::MAX});
        fields["idempotency_key"] = text();
    }
    let description = match action {
        Action::CreateBomRevision => "创建物料清单版本",
        Action::GetBomRevision => "读取物料清单版本",
        Action::SetBomComponents => {
            required.push("components");
            fields["components"] = component_schema();
            "整体替换草稿物料清单组件"
        }
        Action::ValidateRevision => {
            required.push("revision_type");
            fields["revision_type"] = json!({"type":"string","enum":["bom"]});
            "校验物料清单版本的发布条件"
        }
        _ => unreachable!("BOM action"),
    };
    let result = match action {
        Action::ValidateRevision => object(
            &["ok", "run_id", "valid"],
            json!({
            "ok":{"const":true},"run_id":text(),"valid":{"const":true}}),
        ),
        _ => {
            let mut required = vec!["ok", "run_id", "bom_revision"];
            let mut fields =
                json!({"ok":{"const":true},"run_id":text(),"bom_revision":revision_schema()});
            if write {
                required.push("cursor");
                fields["cursor"] = json!({"type":"integer","minimum":0});
            }
            object(&required, fields)
        }
    };
    Spec {
        name: action.name(),
        description,
        domain: "inventory",
        effect: if write { "write" } else { "read" },
        input: object(&required, fields),
        output: json!({"oneOf":[result,object(&["ok","code","field"],
            json!({"ok":{"const":false},"code":text(),"field":text()}))]}),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: if write { action.name() } else { "none" },
        errors: &[
            "INVALID_ARGUMENT",
            "ITEM_REVISION_NOT_FOUND",
            "BOM_REVISION_NOT_FOUND",
            "BOM_REVISION_ALREADY_EXISTS",
            "BOM_REVISION_NOT_ACTIVATABLE",
            "BOM_COMPONENTS_REQUIRED",
            "BOM_REVISION_IMMUTABLE",
            "COMPONENT_REVISION_NOT_FOUND",
            "COMPONENT_UNIT_MISMATCH",
            "COMPONENT_BOM_NOT_FOUND",
            "BOM_CYCLE",
            "BOM_DEPENDENCY_NOT_PUBLISHED",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

fn read(tx: &Transaction<'_>, run: &str, item: &str, revision: i64) -> anyhow::Result<Option<Bom>> {
    let row = tx
        .query_row(
            "SELECT status,components FROM master_boms WHERE run=?1 AND item=?2 AND revision=?3",
            params![run, item, revision],
            |row| Ok((row.get(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    row.map(|(status, components)| {
        Ok(Bom {
            item_id: item.into(),
            revision,
            status,
            components: serde_json::from_str(&components)?,
        })
    })
    .transpose()
}

pub(crate) fn item_revision(
    tx: &Transaction<'_>,
    run: &str,
    item: &str,
    revision: i64,
) -> rusqlite::Result<Option<(String, String)>> {
    tx.query_row(
        "SELECT i.unit,r.status FROM master_items i JOIN master_item_revisions r
         ON r.run=i.run AND r.item=i.item WHERE i.run=?1 AND i.item=?2 AND r.revision=?3",
        params![run, item, revision],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

fn check_components(
    tx: &Transaction<'_>,
    run: &str,
    bom: &Bom,
    publish: bool,
) -> anyhow::Result<Result<(), Rejection>> {
    if publish {
        if bom.status == "draft" {
            let newer: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM master_boms WHERE run=?1 AND item=?2 AND revision>=?3 AND status='active')",
                params![run,bom.item_id,bom.revision], |row| row.get(0),
            )?;
            if newer {
                return Ok(Err(reject("BOM_REVISION_NOT_ACTIVATABLE", "revision")));
            }
        }
        if bom.components.is_empty() {
            return Ok(Err(reject("BOM_COMPONENTS_REQUIRED", "components")));
        }
        if item_revision(tx, run, &bom.item_id, bom.revision)?
            .is_none_or(|(_, status)| status == "draft")
        {
            return Ok(Err(reject("BOM_DEPENDENCY_NOT_PUBLISHED", "item_id")));
        }
    }
    for (id, component) in &bom.components {
        if !identifier(id)
            || component.revision == 0
            || !component.quantity.is_finite()
            || component.quantity <= 0.0
        {
            return Ok(Err(Rejection::invalid("components")));
        }
        let number = i64::from(component.revision);
        let Some((unit, status)) = item_revision(tx, run, id, number)? else {
            return Ok(Err(reject("COMPONENT_REVISION_NOT_FOUND", "components")));
        };
        if component.unit != unit {
            return Ok(Err(reject("COMPONENT_UNIT_MISMATCH", "components")));
        }
        if publish && status == "draft" {
            return Ok(Err(reject("BOM_DEPENDENCY_NOT_PUBLISHED", "components")));
        }
        if component.expand_bom {
            let Some(child) = read(tx, run, id, number)? else {
                return Ok(Err(reject("COMPONENT_BOM_NOT_FOUND", "components")));
            };
            if publish && child.status == "draft" {
                return Ok(Err(reject("BOM_DEPENDENCY_NOT_PUBLISHED", "components")));
            }
        }
    }
    Ok(Ok(()))
}

fn has_cycle(tx: &Transaction<'_>, run: &str, item: &str, revision: i64) -> rusqlite::Result<bool> {
    // Reverse traversal detects cycles introduced anywhere below this edited BOM, including paths through draft parents.
    tx.query_row(
        "WITH RECURSIVE ancestors(item,revision) AS (
             SELECT ?2,?3
             UNION
             SELECT b.item,b.revision FROM ancestors a
             JOIN master_boms b ON b.run=?1
             JOIN json_each(b.components) j ON j.key=a.item
             WHERE json_extract(j.value,'$.expand_bom')=1
               AND json_extract(j.value,'$.revision')=a.revision
         ), descendants(item,revision,expand) AS (
             SELECT j.key,json_extract(j.value,'$.revision'),json_extract(j.value,'$.expand_bom')
             FROM master_boms b,json_each(b.components) j
             WHERE b.run=?1 AND b.item=?2 AND b.revision=?3
             UNION
             SELECT j.key,json_extract(j.value,'$.revision'),json_extract(j.value,'$.expand_bom')
             FROM descendants d JOIN master_boms b ON b.run=?1 AND b.item=d.item AND b.revision=d.revision
             JOIN json_each(b.components) j WHERE d.expand=1
         )
         SELECT EXISTS(SELECT 1 FROM ancestors a JOIN descendants d ON a.item=d.item)",
        params![run,item,revision],|row|row.get(0),
    )
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
    if matches!(action, Action::GetBomRevision | Action::ValidateRevision) {
        let Some(bom) = read(&tx, run, item, number)? else {
            return Ok(Err(reject("BOM_REVISION_NOT_FOUND", "revision")));
        };
        if action == Action::GetBomRevision {
            return Ok(Ok(json!({"ok":true,"run_id":run,"bom_revision":bom})));
        }
        return Ok(check_components(&tx, run, &bom, true)?
            .map(|()| json!({"ok":true,"run_id":run,"valid":true})));
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

    if action == Action::CreateBomRevision {
        if item_revision(&tx, run, item, number)?.is_none() {
            return Ok(Err(reject("ITEM_REVISION_NOT_FOUND", "revision")));
        }
        if tx.execute(
            "INSERT INTO master_boms (run,item,revision,components,status)
            VALUES (?1,?2,?3,'{}','draft') ON CONFLICT(run,item,revision) DO NOTHING",
            params![run, item, number],
        )? == 0
        {
            return Ok(Err(reject("BOM_REVISION_ALREADY_EXISTS", "revision")));
        }
    } else {
        let Some(mut bom) = read(&tx, run, item, number)? else {
            return Ok(Err(reject("BOM_REVISION_NOT_FOUND", "revision")));
        };
        if bom.status != "draft" {
            return Ok(Err(reject("BOM_REVISION_IMMUTABLE", "revision")));
        }
        if action == Action::SetBomComponents {
            let Ok(components) =
                serde_json::from_value::<BTreeMap<String, Component>>(args["components"].clone())
            else {
                return Ok(Err(Rejection::invalid("components")));
            };
            if components.len() > MAX_COMPONENTS {
                return Ok(Err(Rejection::invalid("components")));
            }
            bom.components = components;
            if let Err(error) = check_components(&tx, run, &bom, false)? {
                return Ok(Err(error));
            }
            tx.execute(
                "UPDATE master_boms SET components=?4 WHERE run=?1 AND item=?2 AND revision=?3",
                params![run, item, number, serde_json::to_string(&bom.components)?],
            )?;
            if has_cycle(&tx, run, item, number)? {
                return Ok(Err(reject("BOM_CYCLE", "components")));
            }
        } else {
            if let Err(error) = check_components(&tx, run, &bom, true)? {
                return Ok(Err(error));
            }
            tx.execute("UPDATE master_boms SET status='superseded' WHERE run=?1 AND item=?2 AND status='active'",
                params![run,item])?;
            tx.execute(
                "UPDATE master_boms SET status='active' WHERE run=?1 AND item=?2 AND revision=?3",
                params![run, item, number],
            )?;
        }
    }
    let response =
        json!({"ok":true,"run_id":run,"cursor":cursor+1,"bom_revision":read(&tx,run,item,number)?});
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
