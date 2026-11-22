use rusqlite::{OptionalExtension, Row, Transaction, params};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{
    catalog::{Action, Spec, object, text},
    scenario::{Rejection, identifier, reject},
};

const UNITS: &[&str] = &["each", "mm", "m", "g", "kg", "ml", "l"];

#[derive(Serialize)]
struct Item {
    item_id: String,
    base_unit: String,
    active_revision: Option<u32>,
}

impl Item {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            item_id: row.get(0)?,
            base_unit: row.get(1)?,
            active_revision: row.get(2)?,
        })
    }
}

#[derive(Serialize)]
struct Revision {
    item_id: String,
    revision: u32,
    name: String,
    status: String,
}

pub(crate) fn spec(action: Action, allow_bom: bool) -> Spec {
    let id = json!({"type":"string","minLength":1,"maxLength":64,"pattern":"^[A-Za-z0-9_-]+$"});
    let unit = json!({"type":"string","enum":UNITS});
    let revision = json!({"type":"integer","minimum":1,"maximum":u32::MAX});
    let cursor = json!({"type":"integer","minimum":0,"maximum":i64::MAX});
    let item = object(
        &["item_id", "base_unit", "active_revision"],
        json!({
            "item_id":id,"base_unit":unit,"active_revision":{"type":["integer","null"],"minimum":1}
        }),
    );
    let row = object(
        &["item_id", "revision", "name", "status"],
        json!({
            "item_id":id,"revision":revision,"name":text(),"status":{"enum":["draft","active","superseded"]}
        }),
    );
    let mut fields = Map::new();
    let mut required = Vec::new();
    let read = matches!(
        action,
        Action::GetItem | Action::ListItems | Action::GetItemRevision
    );
    if action != Action::ListItems {
        required.push("item_id");
        fields.insert("item_id".into(), id.clone());
    }
    if matches!(
        action,
        Action::GetItemRevision | Action::CreateItemRevision | Action::ActivateRevision
    ) {
        required.push("revision");
        fields.insert("revision".into(), revision);
    }
    if !read {
        required.extend(["expected_cursor", "idempotency_key"]);
        fields.insert("expected_cursor".into(), cursor.clone());
        fields.insert("idempotency_key".into(), text());
    }
    let (description, mut response, errors) = match action {
        Action::ListItems => {
            fields.insert("after_item_id".into(), id);
            fields.insert(
                "limit".into(),
                json!({"type":"integer","minimum":1,"maximum":100}),
            );
            (
                "按编号分页查询物料",
                json!({"items":{"type":"array","items":item},
                "next_after_item_id":{"type":["string","null"]}}),
                &["INVALID_ARGUMENT"][..],
            )
        }
        Action::GetItem => (
            "查询物料及当前版本",
            json!({"item":item}),
            &["INVALID_ARGUMENT", "ITEM_NOT_FOUND"][..],
        ),
        Action::CreateItem => {
            required.push("base_unit");
            fields.insert("base_unit".into(), unit);
            (
                "建立物料及基本单位",
                json!({"item":item}),
                &[
                    "INVALID_ARGUMENT",
                    "ITEM_ALREADY_EXISTS",
                    "VERSION_CONFLICT",
                    "IDEMPOTENCY_CONFLICT",
                ][..],
            )
        }
        Action::GetItemRevision => (
            "查询明确的物料版本",
            json!({"item_revision":row}),
            &["INVALID_ARGUMENT", "ITEM_REVISION_NOT_FOUND"][..],
        ),
        Action::CreateItemRevision => {
            required.push("name");
            fields.insert("name".into(), text());
            (
                "创建不可变物料版本",
                json!({"item_revision":row}),
                &[
                    "INVALID_ARGUMENT",
                    "ITEM_NOT_FOUND",
                    "ITEM_REVISION_ALREADY_EXISTS",
                    "VERSION_CONFLICT",
                    "IDEMPOTENCY_CONFLICT",
                ][..],
            )
        }
        Action::ActivateRevision => {
            required.push("revision_type");
            fields.insert(
                "revision_type".into(),
                json!({"type":"string","enum":if allow_bom {&["item","bom"][..]} else {&["item"][..]}}),
            );
            (
                if allow_bom {
                    "启用主数据版本"
                } else {
                    "启用物料版本"
                },
                json!({"item_revision":row}),
                if allow_bom {
                    &[
                        "INVALID_ARGUMENT",
                        "ITEM_REVISION_NOT_FOUND",
                        "ITEM_REVISION_NOT_ACTIVATABLE",
                        "BOM_REVISION_NOT_FOUND",
                        "BOM_REVISION_IMMUTABLE",
                        "BOM_REVISION_NOT_ACTIVATABLE",
                        "BOM_COMPONENTS_REQUIRED",
                        "COMPONENT_REVISION_NOT_FOUND",
                        "COMPONENT_UNIT_MISMATCH",
                        "COMPONENT_BOM_NOT_FOUND",
                        "BOM_DEPENDENCY_NOT_PUBLISHED",
                        "VERSION_CONFLICT",
                        "IDEMPOTENCY_CONFLICT",
                    ][..]
                } else {
                    &[
                        "INVALID_ARGUMENT",
                        "ITEM_REVISION_NOT_FOUND",
                        "ITEM_REVISION_NOT_ACTIVATABLE",
                        "VERSION_CONFLICT",
                        "IDEMPOTENCY_CONFLICT",
                    ][..]
                },
            )
        }
        _ => unreachable!("item action"),
    };
    response["ok"] = json!({"const":true});
    response["run_id"] = text();
    if !read {
        response["cursor"] = cursor;
    }
    let output_fields = response.as_object().unwrap();
    let output_required: Vec<_> = output_fields.keys().map(String::as_str).collect();
    let mut output = json!({"oneOf":[object(&output_required,response.clone()),
        object(&["ok","code","field"],json!({"ok":{"const":false},"code":text(),"field":text()}))]});
    if action == Action::ActivateRevision && allow_bom {
        output["oneOf"].as_array_mut().unwrap().push(object(
            &["ok", "run_id", "cursor", "bom_revision"],
            json!({"ok":{"const":true},"run_id":text(),"cursor":{"type":"integer","minimum":0},
                "bom_revision":crate::bom::revision_schema()}),
        ));
    }
    Spec {
        name: action.name(),
        description,
        domain: "inventory",
        effect: if read { "read" } else { "write" },
        input: object(&required, Value::Object(fields)),
        output,
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: if read { "none" } else { action.name() },
        errors,
        visibility: "run",
    }
}

fn revision(
    tx: &Transaction<'_>,
    run: &str,
    item: &str,
    revision: i64,
) -> rusqlite::Result<Option<Revision>> {
    tx.query_row(
        "SELECT item,revision,name,status FROM master_item_revisions WHERE run=?1 AND item=?2 AND revision=?3",
        params![run,item,revision],
        |row| Ok(Revision {item_id:row.get(0)?,revision:row.get(1)?,name:row.get(2)?,status:row.get(3)?}),
    ).optional()
}

fn item(tx: &Transaction<'_>, run: &str, id: &str) -> rusqlite::Result<Option<Item>> {
    tx.query_row(
        "SELECT i.item,i.unit,r.revision FROM master_items i LEFT JOIN master_item_revisions r
         ON r.run=i.run AND r.item=i.item AND r.status='active' WHERE i.run=?1 AND i.item=?2",
        params![run, id],
        Item::read,
    )
    .optional()
}

pub(crate) fn invoke(
    tx: Transaction<'_>,
    run: &str,
    action: Action,
    args: &Map<String, Value>,
    cursor: i64,
) -> anyhow::Result<Result<Value, Rejection>> {
    for field in ["item_id", "after_item_id"] {
        if args
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|id| !identifier(id))
        {
            return Ok(Err(Rejection::invalid(field)));
        }
    }
    if action == Action::ListItems {
        let after = args
            .get("after_item_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(20);
        let mut query = tx.prepare(
            "SELECT i.item,i.unit,r.revision FROM master_items i LEFT JOIN master_item_revisions r
             ON r.run=i.run AND r.item=i.item AND r.status='active'
             WHERE i.run=?1 AND i.item>?2 ORDER BY i.item LIMIT ?3",
        )?;
        let mut items = query
            .query_map(params![run, after, limit + 1], Item::read)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next = if items.len() > limit as usize {
            items.pop();
            items.last().map(|item| item.item_id.as_str())
        } else {
            None
        };
        return Ok(Ok(
            json!({"ok":true,"run_id":run,"items":items,"next_after_item_id":next}),
        ));
    }

    let id = args["item_id"].as_str().unwrap();
    let number = args
        .get("revision")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    if action == Action::GetItem {
        return Ok(item(&tx, run, id)?
            .map(|item| json!({"ok":true,"run_id":run,"item":item}))
            .ok_or_else(|| reject("ITEM_NOT_FOUND", "item_id")));
    }
    if action == Action::GetItemRevision {
        return Ok(revision(&tx, run, id, number)?
            .map(|row| json!({"ok":true,"run_id":run,"item_revision":row}))
            .ok_or_else(|| reject("ITEM_REVISION_NOT_FOUND", "revision")));
    }

    let key = args["idempotency_key"].as_str().unwrap();
    let request = json!({"action":action,"arguments":args}).to_string();
    let receipt = tx
        .query_row(
            "SELECT request,response FROM scenario_requests WHERE run=?1 AND key=?2",
            params![run, key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if let Some((original, response)) = receipt {
        return Ok(if request == original {
            Ok(serde_json::from_str(&response)?)
        } else {
            Err(reject("IDEMPOTENCY_CONFLICT", "idempotency_key"))
        });
    }
    if args["expected_cursor"].as_i64() != Some(cursor) {
        return Ok(Err(reject("VERSION_CONFLICT", "expected_cursor")));
    }

    let mut response = match action {
        Action::CreateItem => {
            let unit = args["base_unit"].as_str().unwrap();
            let created = tx.execute(
                "INSERT INTO master_items (run,item,unit) VALUES (?1,?2,?3) ON CONFLICT(run,item) DO NOTHING",
                params![run,id,unit],
            )?;
            if created == 0 {
                return Ok(Err(reject("ITEM_ALREADY_EXISTS", "item_id")));
            }
            json!({"item":Item{item_id:id.into(),base_unit:unit.into(),active_revision:None}})
        }
        Action::CreateItemRevision => {
            let name = args["name"].as_str().unwrap();
            if name.is_empty() || name.trim() != name {
                return Ok(Err(Rejection::invalid("name")));
            }
            if item(&tx, run, id)?.is_none() {
                return Ok(Err(reject("ITEM_NOT_FOUND", "item_id")));
            }
            let created = tx.execute(
                "INSERT INTO master_item_revisions (run,item,revision,name,status)
                 VALUES (?1,?2,?3,?4,'draft') ON CONFLICT(run,item,revision) DO NOTHING",
                params![run, id, number, name],
            )?;
            if created == 0 {
                return Ok(Err(reject("ITEM_REVISION_ALREADY_EXISTS", "revision")));
            }
            json!({"item_revision":Revision{item_id:id.into(),revision:number as u32,name:name.into(),status:"draft".into()}})
        }
        Action::ActivateRevision => {
            let Some(mut row) = revision(&tx, run, id, number)? else {
                return Ok(Err(reject("ITEM_REVISION_NOT_FOUND", "revision")));
            };
            let active = item(&tx, run, id)?.and_then(|item| item.active_revision);
            if row.status != "draft" || active.is_some_and(|active| i64::from(active) >= number) {
                return Ok(Err(reject("ITEM_REVISION_NOT_ACTIVATABLE", "revision")));
            }
            tx.execute(
                "UPDATE master_item_revisions SET status='superseded'
                WHERE run=?1 AND item=?2 AND status='active'",
                params![run, id],
            )?;
            tx.execute(
                "UPDATE master_item_revisions SET status='active'
                WHERE run=?1 AND item=?2 AND revision=?3",
                params![run, id, number],
            )?;
            row.status = "active".into();
            json!({"item_revision":row})
        }
        _ => unreachable!("item write action"),
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
