use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Transaction, params};
use serde_json::{Value, json};

use crate::{
    catalog::{Spec, object, text},
    inventory::{QUANTITY_SCALE, balance, id_schema, parse_quantity, quantity, quantity_schema},
    mes::Report,
    wip,
};

pub(crate) fn spec() -> Spec {
    let ids = json!({"type":"array","items":id_schema()});
    let units = json!({"type":"integer","minimum":0});
    let issues = object(
        &[
            "reservation_id",
            "lot_id",
            "item_id",
            "revision",
            "issue_location",
            "unit",
            "issued",
            "returned",
            "net_issued",
            "consumed",
            "unused",
            "lot_on_hand",
            "held",
            "reversed",
        ],
        json!({"reservation_id":id_schema(),"lot_id":id_schema(),"item_id":id_schema(),
            "revision":{"type":"integer","minimum":1},"issue_location":id_schema(),"unit":text(),
            "issued":quantity_schema(),"returned":quantity_schema(),"net_issued":quantity_schema(),"consumed":quantity_schema(),
            "unused":quantity_schema(),"lot_on_hand":quantity_schema(),"held":{"type":"boolean"},"reversed":{"type":"boolean"}}),
    );
    let receipts = object(
        &[
            "receipt_id",
            "lot_id",
            "item_id",
            "revision",
            "posting_location",
            "quantity",
            "lot_on_hand",
            "serial_numbers",
            "reversed",
        ],
        json!({"receipt_id":id_schema(),"lot_id":id_schema(),"item_id":id_schema(),"revision":{"type":"integer","minimum":1},
            "posting_location":id_schema(),"quantity":quantity_schema(),"lot_on_hand":quantity_schema(),
            "serial_numbers":ids,"reversed":{"type":"boolean"}}),
    );
    let mut spec = wip::reconcile_spec();
    spec.description = "核对工单实际发退料、在制与完工入库";
    spec.output["oneOf"][0]["properties"]["settlement"] = object(
        &[
            "cursor",
            "snapshot_fingerprint",
            "quality",
            "completion_confirmed",
            "closed",
            "production",
            "material_issues",
            "output_receipts",
            "differences",
            "blocking",
        ],
        json!({"cursor":units,
            "snapshot_fingerprint":{"type":["string","null"]},
            "quality":{"enum":["pending","released","WORK_ORDER_HELD","QUALITY_HOLD","QUALITY_SCRAPPED"]},
            "completion_confirmed":{"type":"boolean"},"closed":{"type":"boolean"},
            "production":object(&["machined","assembled","wip_open","wip_consumed","received",
                "wip_open_ids","effective_report_ids","reversed_report_ids"],
                json!({"machined":units,"assembled":units,"wip_open":units,"wip_consumed":units,"received":units,
                    "wip_open_ids":ids,"effective_report_ids":ids,"reversed_report_ids":ids})),
            "material_issues":{"type":"array","items":issues},"output_receipts":{"type":"array","items":receipts},
            "differences":{"type":"array","items":{"enum":["MATERIAL_ISSUE_MISMATCH","WIP_OUTPUT_MISMATCH",
                "ASSEMBLY_INPUT_MISMATCH","OUTPUT_RECEIPT_MISMATCH"]}},
            "blocking":{"type":"array","items":{"enum":["ORDER_SNAPSHOT_NOT_FOUND","QUALITY_HOLD",
                "QUALITY_SCRAPPED","WORK_ORDER_HELD","MATERIAL_HELD","COMPLETION_REQUIRED","OUTPUT_RECEIPT_REQUIRED"]}}
        }),
    );
    spec
}

fn material_issues(
    tx: &Transaction<'_>,
    run: &str,
    order: &str,
    reports: &[Report],
) -> rusqlite::Result<(Vec<Value>, bool, bool)> {
    let mut actual = BTreeMap::new();
    for input in reports
        .iter()
        .filter(|report| report.order == order && !report.reversed)
        .flat_map(|report| &report.material_inputs)
    {
        *actual.entry(input.reservation_id.as_str()).or_insert(0) +=
            parse_quantity(&input.quantity).expect("validated material actual");
    }
    let mut query = tx.prepare(
        "SELECT r.reservation,r.lot,s.item,s.revision,s.unit,r.quantity,
            COALESCE(t.quantity,0),r.status='issued',
            EXISTS(SELECT 1 FROM stock_effective_lot_holds h WHERE h.run=r.run AND h.batch=r.lot),
            COALESCE((SELECT m.destination FROM stock_whole_transfers m WHERE m.run=i.run AND m.lot=r.lot
              AND m.posted_cursor<i.issued_cursor ORDER BY m.posted_cursor DESC LIMIT 1),s.location)
         FROM stock_operation_issues i
         JOIN stock_reservations r ON r.run=i.run AND r.reservation=i.reservation
         JOIN stock_lot_origins s ON s.run=r.run AND s.lot=r.lot
         LEFT JOIN stock_effective_return_totals t ON t.run=r.run AND t.reservation=r.reservation
         WHERE i.run=?1 AND r.work_order=?2 ORDER BY r.reservation",
    )?;
    let mut rows = query.query(params![run, order])?;
    let mut result = Vec::new();
    let mut mismatch = false;
    let mut held = false;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let lot: String = row.get(1)?;
        let item: String = row.get(2)?;
        let revision: i64 = row.get(3)?;
        let issued: i64 = row.get(5)?;
        let returned: i64 = row.get(6)?;
        let effective: bool = row.get(7)?;
        let lot_held: bool = row.get(8)?;
        let consumed = actual.remove(id.as_str()).unwrap_or(0);
        let net_issued = if effective { issued - returned } else { 0 };
        let unused = net_issued - consumed;
        mismatch |= unused < 0;
        held |= effective && lot_held && issued > returned;
        let (on_hand, _, _, _) = balance(tx, run, &item, Some(revision), None, Some(&lot))?;
        result.push(json!({"reservation_id":id,"lot_id":lot,"item_id":item,"revision":revision,
            "issue_location":row.get::<_,String>(9)?,"unit":row.get::<_,String>(4)?,"issued":quantity(issued),
            "returned":quantity(returned),"net_issued":quantity(net_issued.max(0)),"consumed":quantity(consumed),"unused":quantity(unused.max(0)),
            "lot_on_hand":quantity(on_hand),"held":lot_held,"reversed":!effective}));
    }
    Ok((result, mismatch || !actual.is_empty(), held))
}

pub(crate) fn read(
    tx: &Transaction<'_>,
    run: &str,
    order: &str,
    reports: &[Report],
    quality: &str,
    completion_confirmed: bool,
    closed: bool,
    cursor: i64,
) -> anyhow::Result<Value> {
    let snapshot = crate::snapshots::load(tx, run, order)?;
    let mut differences = Vec::new();
    let mut blocking = Vec::new();
    if snapshot.is_none() {
        blocking.push("ORDER_SNAPSHOT_NOT_FOUND");
    }
    if matches!(
        quality,
        "QUALITY_HOLD" | "QUALITY_SCRAPPED" | "WORK_ORDER_HELD"
    ) {
        blocking.push(quality);
    }
    if !completion_confirmed {
        blocking.push("COMPLETION_REQUIRED");
    }
    let (materials, material_mismatch, material_held) = material_issues(tx, run, order, reports)?;
    if material_held {
        blocking.push("MATERIAL_HELD");
    }
    if material_mismatch {
        differences.push("MATERIAL_ISSUE_MISMATCH");
    }

    let (active, reversed): (Vec<_>, Vec<_>) = reports
        .iter()
        .filter(|report| report.order == order)
        .partition(|report| !report.reversed);
    let production = |step| {
        let matching = || {
            active
                .iter()
                .filter(|report| report.step == step && report.wip_sources.is_empty())
        };
        (
            matching().map(|report| report.quantity).sum::<usize>(),
            matching()
                .flat_map(|report| report.serial_numbers.iter().map(String::as_str))
                .collect::<BTreeSet<_>>(),
        )
    };
    let (machined, machined_serials) = production(0);
    let (assembled, assembled_serials) = production(1);
    let mut wip_open = 0;
    let mut wip_consumed = 0;
    let mut wip_open_ids = Vec::new();
    let mut wip_serials = BTreeSet::new();
    let mut consumed_serials = BTreeSet::new();
    for lot in wip::lots(reports, order).filter(|lot| lot.inactive().is_none()) {
        wip_serials.extend(lot.serials().iter().map(String::as_str));
        if wip::consumer(reports, order, lot.id()).is_some() {
            wip_consumed += lot.quantity();
            consumed_serials.extend(lot.serials().iter().map(String::as_str));
        } else {
            wip_open += lot.quantity();
            wip_open_ids.push(lot.id());
        }
    }
    if machined != wip_open + wip_consumed
        || machined != wip_serials.len()
        || wip_serials != machined_serials
    {
        differences.push("WIP_OUTPUT_MISMATCH");
    }
    if assembled != wip_consumed
        || assembled != consumed_serials.len()
        || consumed_serials != assembled_serials
    {
        differences.push("ASSEMBLY_INPUT_MISMATCH");
    }

    let mut output = tx.prepare(
        "SELECT s.receipt,s.lot,s.item,s.revision,s.location,s.quantity,o.serial_numbers,r.receipt IS NOT NULL
         FROM stock_work_order_output o JOIN stock_receipts s ON s.run=o.run AND s.receipt=o.receipt
         LEFT JOIN stock_receipt_reversals r ON r.run=o.run AND r.receipt=o.receipt
         WHERE o.run=?1 AND o.work_order=?2 ORDER BY s.posted_cursor",
    )?;
    let mut rows = output.query(params![run, order])?;
    let mut receipts = Vec::new();
    let mut received = 0;
    let mut active_receipts = 0;
    let mut receipt_mismatch = false;
    while let Some(row) = rows.next()? {
        let lot: String = row.get(1)?;
        let item: String = row.get(2)?;
        let revision: i64 = row.get(3)?;
        let posted: i64 = row.get(5)?;
        let serials: Vec<String> = serde_json::from_str(&row.get::<_, String>(6)?)?;
        let reversed: bool = row.get(7)?;
        if !reversed {
            active_receipts += 1;
            received += posted;
            let item_snapshot = snapshot.as_ref().map(|snapshot| &snapshot["item"]);
            receipt_mismatch |= posted != assembled as i64 * QUANTITY_SCALE
                || assembled != serials.len()
                || serials.iter().map(String::as_str).collect::<BTreeSet<_>>() != assembled_serials
                || item_snapshot.is_none_or(|snapshot| {
                    snapshot["item_id"] != item || snapshot["revision"] != revision
                });
        }
        let (on_hand, _, _, _) = balance(tx, run, &item, Some(revision), None, Some(&lot))?;
        receipts.push(json!({"receipt_id":row.get::<_,String>(0)?,"lot_id":lot,"item_id":item,"revision":revision,
            "posting_location":row.get::<_,String>(4)?,"quantity":quantity(posted),"lot_on_hand":quantity(on_hand),
            "serial_numbers":serials,"reversed":reversed}));
    }
    if receipt_mismatch || active_receipts > 1 {
        differences.push("OUTPUT_RECEIPT_MISMATCH");
    }
    if completion_confirmed && quality != "QUALITY_SCRAPPED" && active_receipts == 0 {
        blocking.push("OUTPUT_RECEIPT_REQUIRED");
    }
    Ok(
        json!({"cursor":cursor,"snapshot_fingerprint":snapshot.as_ref().map(|snapshot| &snapshot["fingerprint"]),
        "quality":quality,"completion_confirmed":completion_confirmed,"closed":closed,
        "production":{"machined":machined,"assembled":assembled,"wip_open":wip_open,"wip_consumed":wip_consumed,
            "received":received / QUANTITY_SCALE,"wip_open_ids":wip_open_ids,
            "effective_report_ids":active.iter().map(|report| &report.id).collect::<Vec<_>>(),
            "reversed_report_ids":reversed.iter().map(|report| &report.id).collect::<Vec<_>>()},
        "material_issues":materials,"output_receipts":receipts,"differences":differences,"blocking":blocking}),
    )
}
