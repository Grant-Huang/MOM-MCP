use rusqlite::{OptionalExtension, Row, Transaction, params};
use serde_json::{Map, Value, json};

use crate::{
    catalog::{Action, Spec, object, text},
    scenario::{Rejection, identifier, reject},
};

const LOCATIONS: &[&str] = &["RAW-01", "RAW-02"];
const QUANTITY_SCALE: i64 = 1000;
const MAX_RECEIPT_QUANTITY: i64 = 1_000_000_000 * QUANTITY_SCALE;

fn id_schema() -> Value {
    json!({"type":"string","minLength":1,"maxLength":64,"pattern":"^[A-Za-z0-9_-]+$"})
}

fn quantity_schema() -> Value {
    json!({"type":"string","minLength":1,"maxLength":32,"pattern":"^[0-9]+(\\.[0-9]{1,3})?$"})
}

fn revision_schema() -> Value {
    json!({"type":"integer","minimum":1,"maximum":u32::MAX})
}

fn receipt_schema() -> Value {
    object(
        &[
            "receipt_id",
            "item_id",
            "revision",
            "unit",
            "quantity",
            "requires_inspection",
            "status",
            "lot_id",
            "supplier_lot",
            "location",
            "posted_cursor",
            "availability",
        ],
        json!({"receipt_id":id_schema(),"item_id":id_schema(),"revision":revision_schema(),
            "unit":text(),"quantity":quantity_schema(),"requires_inspection":{"type":"boolean"},
            "status":{"enum":["draft","tracked","posted"]},"lot_id":{"type":["string","null"]},
            "supplier_lot":{"type":["string","null"]},"location":{"type":["string","null"]},
            "posted_cursor":{"type":["integer","null"],"minimum":1},
            "availability":{"enum":[null,"pending_inspection","available"]},
            "released_inspection_id":id_schema(),"released_cursor":{"type":"integer","minimum":1}}),
    )
}

pub(crate) fn spec(action: Action) -> Spec {
    let mut fields = json!({});
    let mut required = Vec::new();
    let write = matches!(
        action,
        Action::CreateReceipt
            | Action::AssignReceiptTracking
            | Action::PostReceipt
            | Action::ReleaseReceiptInspection
    );
    if write {
        required.extend(["receipt_id", "expected_cursor", "idempotency_key"]);
        fields["receipt_id"] = id_schema();
        fields["expected_cursor"] = json!({"type":"integer","minimum":0,"maximum":i64::MAX});
        fields["idempotency_key"] = text();
    }
    if matches!(
        action,
        Action::GetStockBalance
            | Action::GetMaterialAvailability
            | Action::ListLots
            | Action::ListInventoryMovements
    ) {
        fields["item_id"] = id_schema();
        fields["revision"] = revision_schema();
        fields["location"] = id_schema();
    }
    if matches!(action, Action::ListLots | Action::ListInventoryMovements) {
        fields["after_receipt_id"] = id_schema();
        fields["limit"] = json!({"type":"integer","minimum":1,"maximum":100});
    }
    let (description, mut output) = match action {
        Action::ListStockLocations => (
            "查询库存库位",
            json!({"locations":{"type":"array","items":id_schema()}}),
        ),
        Action::GetMaterialRequirements => {
            required.push("work_order");
            fields["work_order"] = id_schema();
            (
                "按冻结 BOM 展开物料需求",
                json!({
                "work_order":id_schema(),"snapshot_fingerprint":text(),
                "requirements":{"type":"array","items":object(
                    &["item_id","revision","unit","required","available","shortage"],
                    json!({"item_id":id_schema(),"revision":revision_schema(),"unit":text(),
                        "required":quantity_schema(),"available":quantity_schema(),"shortage":quantity_schema()}))}}),
            )
        }
        Action::GetStockBalance | Action::GetMaterialAvailability => {
            required.push("item_id");
            if action == Action::GetMaterialAvailability {
                required.push("revision");
            }
            (
                "按物料及可选版本、库位查询库存余额",
                json!({"balance":object(
                &["item_id","revision","location","unit","on_hand","pending_inspection","available"],
                json!({"item_id":id_schema(),"revision":{"type":["integer","null"]},
                    "location":{"type":["string","null"]},"unit":text(),
                    "on_hand":quantity_schema(),"pending_inspection":quantity_schema(),
                    "available":quantity_schema()}))}),
            )
        }
        Action::ListLots | Action::ListInventoryMovements => {
            let (description, key) = if action == Action::ListLots {
                ("分页查询已入库批次", "lots")
            } else {
                ("分页查询收货入库流水", "movements")
            };
            let mut output = json!({"next_after_receipt_id":{"type":["string","null"]}});
            output[key] = json!({"type":"array","items":receipt_schema()});
            (description, output)
        }
        Action::GetLot => {
            required.push("lot_id");
            fields["lot_id"] = id_schema();
            ("按批次编号查询入库追踪", json!({"lot":receipt_schema()}))
        }
        Action::CreateReceipt => {
            required.extend(["item_id", "revision", "quantity", "requires_inspection"]);
            fields["item_id"] = id_schema();
            fields["revision"] = revision_schema();
            fields["quantity"] = quantity_schema();
            fields["requires_inspection"] = json!({"type":"boolean"});
            ("创建待过账收货单", json!({"receipt":receipt_schema()}))
        }
        Action::AssignReceiptTracking => {
            required.extend(["lot_id", "supplier_lot", "location"]);
            for field in ["lot_id", "supplier_lot", "location"] {
                fields[field] = id_schema();
            }
            (
                "分配内部批次、供应商批次和库位",
                json!({"receipt":receipt_schema()}),
            )
        }
        Action::ReleaseReceiptInspection => {
            required.push("inspection_id");
            fields["inspection_id"] = id_schema();
            (
                "按合格来料检验放行收货",
                json!({"receipt":receipt_schema()}),
            )
        }
        Action::PostReceipt => ("收货过账并登记库存", json!({"receipt":receipt_schema()})),
        _ => unreachable!("receipt action"),
    };
    output["ok"] = json!({"const":true});
    output["run_id"] = text();
    if write {
        output["cursor"] = json!({"type":"integer","minimum":0});
    }
    let output_required: Vec<_> = output
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    Spec {
        name: action.name(),
        description,
        domain: "inventory",
        effect: if write { "write" } else { "read" },
        input: object(&required, fields),
        output: json!({"oneOf":[object(&output_required,output.clone()),
            object(&["ok","code","field"],json!({"ok":{"const":false},"code":text(),"field":text()}))]}),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: if write { action.name() } else { "none" },
        errors: &[
            "INVALID_ARGUMENT",
            "ITEM_NOT_FOUND",
            "ITEM_REVISION_NOT_FOUND",
            "ITEM_REVISION_NOT_PUBLISHED",
            "RECEIPT_ALREADY_EXISTS",
            "RECEIPT_NOT_FOUND",
            "RECEIPT_ALREADY_POSTED",
            "RECEIPT_TRACKING_REQUIRED",
            "LOCATION_NOT_FOUND",
            "LOT_ALREADY_ASSIGNED",
            "LOT_NOT_FOUND",
            "VERSION_CONFLICT",
            "IDEMPOTENCY_CONFLICT",
            "RECEIPT_INSPECTION_NOT_PENDING",
            "RECEIPT_INSPECTION_NOT_PASSED",
            "ORDER_SNAPSHOT_NOT_FOUND",
            "BOM_QUANTITY_UNSUPPORTED",
            "BOM_EXPANSION_LIMIT",
        ],
        visibility: "run",
    }
}

fn parse_quantity(value: &str) -> Option<i64> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || (value.contains('.') && fraction.is_empty())
        || fraction.len() > 3
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let whole = whole.parse::<i64>().ok()?.checked_mul(QUANTITY_SCALE)?;
    let fraction_milli =
        fraction.parse::<i64>().unwrap_or(0) * 10_i64.pow((3 - fraction.len()) as u32);
    whole
        .checked_add(fraction_milli)
        .filter(|q| (1..=MAX_RECEIPT_QUANTITY).contains(q))
}

fn quantity(milli: i64) -> String {
    let whole = milli / QUANTITY_SCALE;
    let remainder = milli % QUANTITY_SCALE;
    if remainder == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{remainder:03}")
            .trim_end_matches('0')
            .to_owned()
    }
}

fn record(row: &Row<'_>) -> rusqlite::Result<Value> {
    let inspection = row.get::<_, bool>(5)?;
    let lot = row.get::<_, Option<String>>(6)?;
    let posted = row.get::<_, Option<i64>>(9)?;
    let status = if posted.is_some() {
        "posted"
    } else if lot.is_some() {
        "tracked"
    } else {
        "draft"
    };
    let released: Option<String> = row.get(10)?;
    let availability = posted.map(|_| {
        if inspection && released.is_none() {
            "pending_inspection"
        } else {
            "available"
        }
    });
    let mut response = json!({"receipt_id":row.get::<_,String>(0)?,"item_id":row.get::<_,String>(1)?,
        "revision":row.get::<_,i64>(2)?,"unit":row.get::<_,String>(3)?,"quantity":quantity(row.get(4)?),
        "requires_inspection":inspection,"lot_id":lot,"supplier_lot":row.get::<_,Option<String>>(7)?,
        "location":row.get::<_,Option<String>>(8)?,"posted_cursor":posted,"status":status,
        "availability":availability});
    if let Some(released) = released {
        response["released_inspection_id"] = json!(released);
        response["released_cursor"] = json!(row.get::<_, i64>(11)?);
    }
    Ok(response)
}

const RECEIPT_COLUMNS: &str = "receipt,item,revision,unit,quantity,requires_inspection,lot,supplier_lot,location,posted_cursor,
    (SELECT inspection FROM stock_receipt_releases r WHERE r.run=stock_receipts.run AND r.receipt=stock_receipts.receipt),
    (SELECT released_cursor FROM stock_receipt_releases r WHERE r.run=stock_receipts.run AND r.receipt=stock_receipts.receipt)";

fn read(tx: &Transaction<'_>, run: &str, id: &str) -> rusqlite::Result<Option<Value>> {
    tx.query_row(
        &format!("SELECT {RECEIPT_COLUMNS} FROM stock_receipts WHERE run=?1 AND receipt=?2"),
        params![run, id],
        record,
    )
    .optional()
}

fn query(
    tx: &Transaction<'_>,
    run: &str,
    action: Action,
    args: &Map<String, Value>,
) -> anyhow::Result<Result<Value, Rejection>> {
    let item = args.get("item_id").and_then(Value::as_str);
    let revision = args.get("revision").and_then(Value::as_i64);
    let location = args.get("location").and_then(Value::as_str);
    let fields = match action {
        Action::ListStockLocations => json!({"locations":LOCATIONS}),
        Action::GetLot => {
            let lot = tx
                .query_row(
                    &format!(
                        "SELECT {RECEIPT_COLUMNS} FROM stock_receipts
                WHERE run=?1 AND lot=?2 AND posted_cursor IS NOT NULL"
                    ),
                    params![run, args["lot_id"].as_str().unwrap()],
                    record,
                )
                .optional()?;
            let Some(lot) = lot else {
                return Ok(Err(reject("LOT_NOT_FOUND", "lot_id")));
            };
            json!({"lot":lot})
        }
        Action::GetStockBalance | Action::GetMaterialAvailability => {
            let unit: Option<String> = tx
                .query_row(
                    "SELECT unit FROM master_items WHERE run=?1 AND item=?2",
                    params![run, item],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(unit) = unit else {
                return Ok(Err(reject("ITEM_NOT_FOUND", "item_id")));
            };
            if let Some(revision) = revision
                && crate::bom::item_revision(tx, run, item.unwrap(), revision)?.is_none()
            {
                return Ok(Err(reject("ITEM_REVISION_NOT_FOUND", "revision")));
            }
            let (on_hand, pending) = balance(tx, run, item.unwrap(), revision, location)?;
            json!({"balance":{"item_id":item,"revision":revision,"location":location,"unit":unit,
                "on_hand":quantity(on_hand),"pending_inspection":quantity(pending),"available":quantity(on_hand-pending)}})
        }
        _ => {
            let after = args
                .get("after_receipt_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(20);
            let mut statement = tx.prepare(&format!(
                "SELECT {RECEIPT_COLUMNS} FROM stock_receipts
                WHERE run=?1 AND receipt>?2 AND posted_cursor IS NOT NULL
                AND (?3 IS NULL OR item=?3) AND (?4 IS NULL OR revision=?4)
                AND (?5 IS NULL OR location=?5) ORDER BY receipt LIMIT ?6"
            ))?;
            let mut rows = statement
                .query_map(
                    params![run, after, item, revision, location, limit + 1],
                    record,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let next = if rows.len() > limit as usize {
                rows.pop();
                rows.last().map(|row| row["receipt_id"].clone())
            } else {
                None
            };
            let mut result = json!({"next_after_receipt_id":next});
            result[if action == Action::ListLots {
                "lots"
            } else {
                "movements"
            }] = json!(rows);
            result
        }
    };
    let mut response = fields;
    response["ok"] = json!(true);
    response["run_id"] = json!(run);
    Ok(Ok(response))
}

pub(crate) fn invoke(
    tx: Transaction<'_>,
    run: &str,
    action: Action,
    args: &Map<String, Value>,
    cursor: i64,
) -> anyhow::Result<Result<Value, Rejection>> {
    for field in [
        "item_id",
        "receipt_id",
        "inspection_id",
        "lot_id",
        "supplier_lot",
        "location",
        "after_receipt_id",
    ] {
        if args
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|id| !identifier(id))
        {
            return Ok(Err(Rejection::invalid(field)));
        }
    }
    if args
        .get("location")
        .and_then(Value::as_str)
        .is_some_and(|id| !LOCATIONS.contains(&id))
    {
        return Ok(Err(reject("LOCATION_NOT_FOUND", "location")));
    }
    if !matches!(
        action,
        Action::CreateReceipt
            | Action::AssignReceiptTracking
            | Action::PostReceipt
            | Action::ReleaseReceiptInspection
    ) {
        return query(&tx, run, action, args);
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
    let receipt = args["receipt_id"].as_str().unwrap();
    if action == Action::ReleaseReceiptInspection {
        if !pending_receipt(&tx, run, receipt)? {
            return Ok(Err(reject("RECEIPT_INSPECTION_NOT_PENDING", "receipt_id")));
        }
        let inspection = args["inspection_id"].as_str().unwrap();
        let passed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM stock_receipt_inspections link
             JOIN quality_inspection_results q ON q.run=link.run AND q.inspection=link.inspection
             WHERE link.run=?1 AND link.receipt=?2 AND link.inspection=?3 AND q.status='passed'
             AND NOT EXISTS(
                 SELECT 1 FROM stock_receipt_inspections newer
                 JOIN quality_inspection_results result ON result.run=newer.run AND result.inspection=newer.inspection
                 WHERE newer.run=link.run AND newer.receipt=link.receipt AND result.status='failed' AND result.rowid>q.rowid
             ))",
            params![run,receipt,inspection],|row| row.get(0),
        )?;
        if !passed {
            return Ok(Err(reject(
                "RECEIPT_INSPECTION_NOT_PASSED",
                "inspection_id",
            )));
        }
        tx.execute("INSERT INTO stock_receipt_releases (run,receipt,inspection,released_cursor) VALUES (?1,?2,?3,?4)",
            params![run,receipt,inspection,cursor+1])?;
    } else if action == Action::CreateReceipt {
        let item = args["item_id"].as_str().unwrap();
        let revision = args["revision"].as_i64().unwrap();
        let Some((unit, status)) = crate::bom::item_revision(&tx, run, item, revision)? else {
            return Ok(Err(reject("ITEM_REVISION_NOT_FOUND", "revision")));
        };
        if status == "draft" {
            return Ok(Err(reject("ITEM_REVISION_NOT_PUBLISHED", "revision")));
        }
        let Some(amount) = parse_quantity(args["quantity"].as_str().unwrap())
            .filter(|q| unit != "each" || q % QUANTITY_SCALE == 0)
        else {
            return Ok(Err(Rejection::invalid("quantity")));
        };
        if tx.execute("INSERT INTO stock_receipts (run,receipt,item,revision,unit,quantity,requires_inspection)
            VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(run,receipt) DO NOTHING",
            params![run,receipt,item,revision,unit,amount,args["requires_inspection"].as_bool().unwrap()])? == 0 {
            return Ok(Err(reject("RECEIPT_ALREADY_EXISTS","receipt_id")));
        }
    } else {
        let Some(row) = read(&tx, run, receipt)? else {
            return Ok(Err(reject("RECEIPT_NOT_FOUND", "receipt_id")));
        };
        if row["status"] == "posted" {
            return Ok(Err(reject("RECEIPT_ALREADY_POSTED", "receipt_id")));
        }
        if action == Action::AssignReceiptTracking {
            let lot = args["lot_id"].as_str().unwrap();
            let assigned: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM stock_receipts
                WHERE run=?1 AND lot=?2 AND receipt<>?3)",
                params![run, lot, receipt],
                |row| row.get(0),
            )?;
            if assigned {
                return Ok(Err(reject("LOT_ALREADY_ASSIGNED", "lot_id")));
            }
            tx.execute("UPDATE stock_receipts SET lot=?3,supplier_lot=?4,location=?5 WHERE run=?1 AND receipt=?2",
                params![run,receipt,lot,args["supplier_lot"].as_str().unwrap(),args["location"].as_str().unwrap()])?;
        } else {
            if row["status"] != "tracked" {
                return Ok(Err(reject("RECEIPT_TRACKING_REQUIRED", "receipt_id")));
            }
            tx.execute(
                "UPDATE stock_receipts SET posted_cursor=?3 WHERE run=?1 AND receipt=?2",
                params![run, receipt, cursor + 1],
            )?;
        }
    }
    let response =
        json!({"ok":true,"run_id":run,"cursor":cursor+1,"receipt":read(&tx,run,receipt)?});
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

pub(crate) fn pending_receipt(
    tx: &Transaction<'_>,
    run: &str,
    receipt: &str,
) -> rusqlite::Result<bool> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM stock_receipts s WHERE s.run=?1 AND s.receipt=?2
         AND s.posted_cursor IS NOT NULL AND s.requires_inspection
         AND NOT EXISTS(SELECT 1 FROM stock_receipt_releases r WHERE r.run=s.run AND r.receipt=s.receipt))",
        params![run,receipt],|row|row.get(0),
    )
}

fn balance(
    tx: &Transaction<'_>,
    run: &str,
    item: &str,
    revision: Option<i64>,
    location: Option<&str>,
) -> rusqlite::Result<(i64, i64)> {
    tx.query_row(
        "SELECT COALESCE(SUM(s.quantity),0),
         COALESCE(SUM(CASE WHEN s.requires_inspection AND r.receipt IS NULL THEN s.quantity ELSE 0 END),0)
         FROM stock_receipts s LEFT JOIN stock_receipt_releases r ON r.run=s.run AND r.receipt=s.receipt
         WHERE s.run=?1 AND s.item=?2 AND s.posted_cursor IS NOT NULL
         AND (?3 IS NULL OR s.revision=?3) AND (?4 IS NULL OR s.location=?4)",
        params![run,item,revision,location],|row|Ok((row.get(0)?,row.get(1)?)),
    )
}

#[derive(Default)]
struct Requirements {
    quantities: std::collections::BTreeMap<(String, i64, String), i64>,
    visited: usize,
}

impl Requirements {
    fn expand(
        &mut self,
        tx: &Transaction<'_>,
        run: &str,
        bom: &Value,
        factor: i64,
        depth: u8,
    ) -> anyhow::Result<Result<(), Rejection>> {
        for (id, component) in bom["components"].as_object().expect("published components") {
            self.visited += 1;
            if self.visited > 1000 || depth > 64 {
                return Ok(Err(reject("BOM_EXPANSION_LIMIT", "work_order")));
            }
            let revision = component["revision"]
                .as_i64()
                .expect("published component revision");
            let unit = component["unit"]
                .as_str()
                .expect("published component unit");
            let Some(per_parent) = parse_quantity(&component["quantity"].to_string()) else {
                return Ok(Err(reject("BOM_QUANTITY_UNSUPPORTED", "work_order")));
            };
            let product = i128::from(factor) * i128::from(per_parent);
            if product % i128::from(QUANTITY_SCALE) != 0 {
                return Ok(Err(reject("BOM_QUANTITY_UNSUPPORTED", "work_order")));
            }
            let Ok(needed) = i64::try_from(product / i128::from(QUANTITY_SCALE)) else {
                return Ok(Err(reject("BOM_QUANTITY_UNSUPPORTED", "work_order")));
            };
            if unit == "each" && needed % QUANTITY_SCALE != 0 {
                return Ok(Err(reject("BOM_QUANTITY_UNSUPPORTED", "work_order")));
            }
            if component["expand_bom"] == true {
                let child = crate::bom::read(tx, run, id, revision)?.expect("published child BOM");
                if let Err(error) =
                    self.expand(tx, run, &serde_json::to_value(child)?, needed, depth + 1)?
                {
                    return Ok(Err(error));
                }
            } else {
                let entry = self
                    .quantities
                    .entry((id.clone(), revision, unit.into()))
                    .or_default();
                let Some(total) = entry.checked_add(needed) else {
                    return Ok(Err(reject("BOM_QUANTITY_UNSUPPORTED", "work_order")));
                };
                *entry = total;
            }
        }
        Ok(Ok(()))
    }
}

pub(crate) fn requirements(
    tx: &Transaction<'_>,
    run: &str,
    order: &str,
    order_quantity: i64,
) -> anyhow::Result<Result<Value, Rejection>> {
    if !identifier(order) {
        return Ok(Err(Rejection::invalid("work_order")));
    }
    let Some(snapshot) = crate::snapshots::load(tx, run, order)? else {
        return Ok(Err(reject("ORDER_SNAPSHOT_NOT_FOUND", "work_order")));
    };
    let mut expansion = Requirements::default();
    if let Err(error) = expansion.expand(
        tx,
        run,
        &snapshot["bom"],
        order_quantity * QUANTITY_SCALE,
        0,
    )? {
        return Ok(Err(error));
    }
    let mut requirements = Vec::with_capacity(expansion.quantities.len());
    for ((item, revision, unit), required) in expansion.quantities {
        let (on_hand, pending) = balance(tx, run, &item, Some(revision), None)?;
        let available = on_hand - pending;
        requirements.push(
            json!({"item_id":item,"revision":revision,"unit":unit,"required":quantity(required),
            "available":quantity(available),"shortage":quantity((required-available).max(0))}),
        );
    }
    Ok(Ok(json!({"ok":true,"run_id":run,"work_order":order,
        "snapshot_fingerprint":snapshot["fingerprint"],"requirements":requirements})))
}
