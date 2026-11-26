use rusqlite::{OptionalExtension, Transaction, params};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    catalog::{Action, Spec, object, text},
    scenario::{Rejection, identifier, reject},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    item_id: String,
    inspection_plan_id: String,
    #[serde(default)]
    pub(crate) configuration: Option<String>,
}

fn selection_schema() -> Value {
    object(
        &["item_id", "inspection_plan_id"],
        json!({"item_id":text(),"inspection_plan_id":text()}),
    )
}

pub(crate) fn release_spec(action: Action) -> Spec {
    let mut spec = crate::mes::spec(action);
    spec.input["properties"]["definition_selection"] = selection_schema();
    spec.input["required"]
        .as_array_mut()
        .unwrap()
        .push(json!("definition_selection"));
    if action == Action::GetReleaseReadiness {
        spec.output["oneOf"][0]["properties"]["definition_snapshot"] = json!({"type":"object"});
        spec.errors = &[
            "INVALID_ARGUMENT",
            "WORK_ORDER_NOT_FOUND",
            "MASTER_DEFINITION_NOT_ACTIVE",
            "BOM_CONFIGURATION_REQUIRED",
            "BOM_CONFIGURATION_EMPTY",
        ];
    } else {
        spec.input["properties"]["snapshot_fingerprint"] =
            json!({"type":"string","minLength":64,"maxLength":64});
        spec.input["required"]
            .as_array_mut()
            .unwrap()
            .push(json!("snapshot_fingerprint"));
        spec.output["properties"]["definition_snapshot"] = json!({"type":"object"});
        spec.errors = &[
            "INVALID_ARGUMENT",
            "WORK_ORDER_NOT_FOUND",
            "INVALID_STATE",
            "MATERIAL_SHORTAGE",
            "MATERIAL_HELD",
            "CELL_NOT_CAPABLE",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
            "SNAPSHOT_CHANGED",
        ];
    }
    spec
}

pub(crate) fn spec(action: Action) -> Spec {
    let (description, required, fields) = if action == Action::GetOrderSnapshot {
        (
            "查询工单下达时冻结的定义",
            vec!["ok", "run_id", "snapshot"],
            json!({"ok":{"const":true},"run_id":text(),"snapshot":{"type":"object"}}),
        )
    } else {
        (
            "比较冻结工单定义与当前工程版本",
            vec![
                "ok",
                "run_id",
                "work_order",
                "frozen",
                "current",
                "changed",
                "requires_review",
            ],
            json!({"ok":{"const":true},"run_id":text(),"work_order":text(),"frozen":{"type":"object"},
                "current":{"type":"object"},"changed":{"type":"array","items":text()},"requires_review":{"type":"boolean"}}),
        )
    };
    Spec {
        name: action.name(),
        description,
        domain: "production",
        effect: "read",
        input: object(&["work_order"], json!({"work_order":text()})),
        output: json!({"oneOf":[object(&required,fields),
            object(&["ok","code","field"],json!({"ok":{"const":false},"code":text(),"field":text()}))]}),
        preconditions: &["run_open", "order_definition_frozen"],
        side_effect: "none",
        errors: &[
            "INVALID_ARGUMENT",
            "ORDER_SNAPSHOT_NOT_FOUND",
            "MASTER_DEFINITION_NOT_ACTIVE",
            "BOM_CONFIGURATION_REQUIRED",
            "BOM_CONFIGURATION_EMPTY",
        ],
        visibility: "run",
    }
}

pub(crate) fn selection(value: &Value) -> Result<Selection, Rejection> {
    let selection: Selection = serde_json::from_value(value.clone())
        .map_err(|_| Rejection::invalid("definition_selection"))?;
    if !identifier(&selection.item_id)
        || !identifier(&selection.inspection_plan_id)
        || selection.configuration.is_some()
    {
        return Err(Rejection::invalid("definition_selection"));
    }
    Ok(selection)
}

pub(crate) fn resolve(
    tx: &Transaction<'_>,
    run: &str,
    order: &str,
    selection: &Selection,
) -> anyhow::Result<Result<Value, Rejection>> {
    let row = tx.query_row(
        "SELECT i.unit,r.revision,r.name,p.characteristics,p.specification,p.revision
         FROM master_items i
         JOIN master_item_revisions r ON r.run=i.run AND r.item=i.item AND r.status='active'
         JOIN master_boms b ON b.run=i.run AND b.item=i.item AND b.revision=r.revision AND b.status='active'
         JOIN master_routes t ON t.run=i.run AND t.item=i.item AND t.revision=r.revision AND t.status='active'
         JOIN quality_inspection_plans p ON p.run=i.run AND p.plan=?3 AND p.status='active'
         WHERE i.run=?1 AND i.item=?2",
        params![run,selection.item_id,selection.inspection_plan_id],
        |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,
            r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,i64>(5)?)),
    ).optional()?;
    let Some((unit, revision, name, characteristics, specification, plan_revision)) = row else {
        return Ok(Err(reject(
            "MASTER_DEFINITION_NOT_ACTIVE",
            "definition_selection",
        )));
    };
    let bom = match crate::bom::read(tx, run, &selection.item_id, revision)?
        .expect("selected BOM")
        .for_configuration(selection.configuration.as_deref())
    {
        Ok(bom) => bom,
        Err(error) => return Ok(Err(error)),
    };
    let route =
        crate::routes::read(tx, run, &selection.item_id, revision)?.expect("selected route");
    let mut snapshot = json!({"work_order":order,
        "definition_selection":{"item_id":selection.item_id,"inspection_plan_id":selection.inspection_plan_id},
        "item":{"item_id":selection.item_id,"revision":revision,"name":name,"base_unit":unit},
        "bom":bom,"route":route,
        "inspection_plan":{"plan_id":selection.inspection_plan_id,"revision":plan_revision,"specification_version":specification,
            "characteristics":serde_json::from_str::<Value>(&characteristics)?}});
    if let Some(configuration) = &selection.configuration {
        snapshot["definition_selection"]["configuration"] = json!(configuration);
    }
    let fingerprint = Sha256::digest(serde_json::to_vec(
        &json!({"run_id":run,"definition":snapshot}),
    )?);
    snapshot["fingerprint"] = json!(
        fingerprint
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    Ok(Ok(snapshot))
}

pub(crate) fn load(tx: &Transaction<'_>, run: &str, order: &str) -> anyhow::Result<Option<Value>> {
    let data: Option<String> = tx
        .query_row(
            "SELECT snapshot FROM order_definition_snapshots WHERE run=?1 AND work_order=?2",
            params![run, order],
            |row| row.get(0),
        )
        .optional()?;
    Ok(data.as_deref().map(serde_json::from_str).transpose()?)
}

pub(crate) fn query(
    tx: &Transaction<'_>,
    run: &str,
    action: Action,
    order: &str,
) -> anyhow::Result<Result<Value, Rejection>> {
    if !identifier(order) {
        return Ok(Err(Rejection::invalid("work_order")));
    }
    let Some(frozen) = load(tx, run, order)? else {
        return Ok(Err(reject("ORDER_SNAPSHOT_NOT_FOUND", "work_order")));
    };
    if action == Action::GetOrderSnapshot {
        return Ok(Ok(json!({"ok":true,"run_id":run,"snapshot":frozen})));
    }
    let selection: Selection = serde_json::from_value(frozen["definition_selection"].clone())?;
    let current = match resolve(tx, run, order, &selection)? {
        Ok(snapshot) => snapshot,
        Err(error) => return Ok(Err(error)),
    };
    let changed: Vec<_> = ["item", "bom", "route", "inspection_plan"]
        .into_iter()
        .filter(|&field| frozen[field] != current[field])
        .collect();
    Ok(Ok(json!({"ok":true,"run_id":run,"work_order":order,
        "frozen":frozen,"current":current,"requires_review":!changed.is_empty(),"changed":changed})))
}

pub(crate) fn freeze(
    tx: &Transaction<'_>,
    run: &str,
    args: &Map<String, Value>,
    snapshot: &Value,
    response: &mut Value,
) -> anyhow::Result<()> {
    tx.execute(
        "INSERT INTO order_definition_snapshots (run,work_order,snapshot) VALUES (?1,?2,?3)",
        params![
            run,
            args["work_order"].as_str().unwrap(),
            snapshot.to_string()
        ],
    )?;
    response["definition_snapshot"] = snapshot.clone();
    tx.execute(
        "UPDATE scenario_requests SET response=?3 WHERE run=?1 AND key=?2",
        params![
            run,
            args["idempotency_key"].as_str().unwrap(),
            response.to_string()
        ],
    )?;
    Ok(())
}
