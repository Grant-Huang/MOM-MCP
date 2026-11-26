use std::collections::{BTreeMap, BTreeSet};

use rusqlite::Transaction;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{
    catalog::{Action, Spec, object, text},
    inventory::{id_schema, quantity_schema},
    mes::Report,
    scenario::identifier,
};

#[derive(Serialize)]
struct Node<'a> {
    kind: &'static str,
    id: std::borrow::Cow<'a, str>,
}

#[derive(Serialize)]
struct Edge<'a> {
    edge_id: String,
    kind: &'static str,
    work_order: &'a str,
    source: Node<'a>,
    target: Node<'a>,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    quantity: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reservation_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    work_center: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    process_record: Option<&'a str>,
}

pub(crate) fn spec(action: Action) -> Spec {
    let mut required = Vec::new();
    let mut fields = json!({"work_order":id_schema(),"after_edge_id":id_schema(),
        "limit":{"type":"integer","minimum":1,"maximum":100}});
    let description = match action {
        Action::TraceLotForward => {
            required.push("lot_id");
            fields["lot_id"] = id_schema();
            "从实际投入批次追溯在制产出、序列号及下游耗用"
        }
        Action::TraceSerialBackward => {
            required.push("serial_number");
            fields["serial_number"] = id_schema();
            "从序列号追溯其来源在制批次和实际投入"
        }
        Action::ListGenealogyEdges => "按工单分页查询投入、产出和耗用谱系",
        _ => unreachable!("genealogy action"),
    };
    let node = object(
        &["kind", "id"],
        json!({"kind":{"enum":["material_lot","wip_lot","serial","operation_report","stock_lot"]},"id":id_schema()}),
    );
    let edge = object(
        &[
            "edge_id",
            "kind",
            "work_order",
            "source",
            "target",
            "status",
        ],
        json!({
        "edge_id":id_schema(),"kind":{"enum":["material_input","serial_output","wip_consumption","wip_split","wip_merge","finished_output_receipt","material_split","operation_output","coating_input","coating_output"]},
        "work_order":id_schema(),"source":node,"target":node,"status":{"enum":["active","reversed"]},
        "reservation_id":id_schema(),"quantity":quantity_schema(),"unit":text(),"operation":id_schema(),"work_center":id_schema(),"process_record":id_schema()}),
    );
    Spec {
        name: action.name(),
        description,
        domain: "production",
        effect: "read",
        input: object(&required, fields),
        output: json!({"oneOf":[object(&["ok","run_id","edges","next_after_edge_id"],
            json!({"ok":{"const":true},"run_id":text(),"edges":{"type":"array","items":edge},
            "next_after_edge_id":{"type":["null","string"]}})),
            object(&["ok","code"],json!({"ok":{"const":false},"code":text()}))]}),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: "none",
        errors: &["INVALID_ARGUMENT", "WORK_ORDER_NOT_FOUND"],
        visibility: "run",
    }
}

fn status(reversed: bool) -> &'static str {
    if reversed { "reversed" } else { "active" }
}

pub(crate) enum Detail {
    Standard,
    Operations,
    MaterialSplits,
}

pub(crate) fn read<'a>(
    tx: &Transaction<'_>,
    run: &str,
    action: Action,
    reports: &'a [Report],
    args: &Map<String, Value>,
    detail: Detail,
) -> anyhow::Result<Value> {
    let selector = |name: &str| args.get(name).and_then(Value::as_str);
    let order = selector("work_order");
    let after = selector("after_edge_id");
    let lot = selector("lot_id");
    let serial = selector("serial_number");
    if [order, after, lot, serial]
        .into_iter()
        .flatten()
        .any(|id| !identifier(id))
    {
        return Ok(json!({"ok":false,"code":"INVALID_ARGUMENT"}));
    }
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
    let mut consumers: BTreeMap<&str, Vec<&Report>> = BTreeMap::new();
    for report in reports
        .iter()
        .filter(|report| order.is_none_or(|order| report.order == order))
    {
        for input in &report.wip_inputs {
            consumers.entry(input).or_default().push(report);
        }
    }

    let operation_detail = matches!(detail, Detail::Operations);
    let mut material_splits = BTreeMap::new();
    if matches!(detail, Detail::MaterialSplits) {
        let mut query = tx.prepare(
            "SELECT p.child,t.lot,t.posted_cursor FROM stock_lot_splits p
             JOIN stock_transfers t ON t.run=p.run AND t.transfer=p.transfer
             JOIN stock_available_lots s ON s.run=p.run AND s.lot=p.child
             WHERE p.run=?1 AND t.posted_cursor IS NOT NULL",
        )?;
        material_splits = query
            .query_map([run], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (row.get::<_, String>(1)?, row.get::<_, i64>(2)?),
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
    }
    let matches_lot = |requested: &str, actual: &str| {
        requested == actual
            || material_splits
                .get(actual)
                .is_some_and(|(parent, _)| parent == requested)
    };
    let receipts = crate::finished_output::genealogy(tx, run, order)?;
    let mut edges = Vec::new();
    for report in reports
        .iter()
        .filter(|report| report.step == 0 && order.is_none_or(|order| report.order == order))
    {
        let consolidated = !report.wip_sources.is_empty();
        let source_serials: BTreeSet<_> = lot
            .filter(|_| consolidated)
            .into_iter()
            .flat_map(|lot| {
                reports.iter().filter(move |source| {
                    report.wip_sources.contains(&source.id)
                        && source
                            .material_inputs
                            .iter()
                            .any(|input| matches_lot(lot, &input.lot_id))
                })
            })
            .flat_map(|source| source.serial_numbers.iter())
            .collect();
        if lot.is_some_and(|lot| {
            if consolidated {
                source_serials.is_empty()
            } else {
                !report
                    .material_inputs
                    .iter()
                    .any(|input| matches_lot(lot, &input.lot_id))
            }
        }) || serial.is_some_and(|serial| {
            !report
                .serial_numbers
                .iter()
                .any(|candidate| candidate == serial)
        }) {
            continue;
        }
        let work_order = report.order.as_str();
        let wip = || Node {
            kind: "wip_lot",
            id: report.id.as_str().into(),
        };
        let append_assembly_outputs = |consumer: &'a Report, input_serials: &[String]| {
            let mut outputs = Vec::new();
            if !operation_detail {
                return outputs;
            }
            for (index, number) in
                consumer
                    .serial_numbers
                    .iter()
                    .enumerate()
                    .filter(|(_, number)| {
                        input_serials.contains(number)
                            && serial.is_none_or(|serial| number.as_str() == serial)
                            && (!consolidated || lot.is_none() || source_serials.contains(*number))
                    })
            {
                outputs.push(Edge {
                    edge_id: format!("{}-operation-serial-{index}", consumer.id),
                    kind: "operation_output",
                    work_order,
                    source: Node {
                        kind: "operation_report",
                        id: consumer.id.as_str().into(),
                    },
                    target: Node {
                        kind: "serial",
                        id: number.as_str().into(),
                    },
                    status: status(consumer.reversed),
                    quantity: None,
                    unit: None,
                    reservation_id: None,
                    operation: Some("assembly"),
                    work_center: None,
                    process_record: None,
                });
            }
            outputs
        };
        let coating_node = |coating: &crate::wip::Coating| Node {
            kind: "operation_report",
            id: format!("coating-{}", coating.cursor).into(),
        };
        if operation_detail {
            if let Some(tool) = &report.tool_setup {
                edges.push(Edge {
                    edge_id: format!("{}-operation-output", report.id),
                    kind: "operation_output",
                    work_order,
                    source: Node {
                        kind: "operation_report",
                        id: report.id.as_str().into(),
                    },
                    target: wip(),
                    status: status(report.reversed),
                    quantity: None,
                    unit: None,
                    reservation_id: None,
                    operation: Some("machining"),
                    work_center: Some(&tool.work_center),
                    process_record: None,
                });
            }
            if let Some(coating) = &report.coating {
                edges.push(Edge {
                    edge_id: format!("{}-coating-input", report.id),
                    kind: "coating_input",
                    work_order,
                    source: wip(),
                    target: coating_node(coating),
                    status: status(report.reversed),
                    quantity: None,
                    unit: None,
                    reservation_id: None,
                    operation: Some("coating"),
                    work_center: Some(&coating.work_center),
                    process_record: Some(&coating.process_record),
                });
            }
        }
        for (index, input) in report.material_inputs.iter().enumerate() {
            if lot.is_some_and(|lot| !matches_lot(lot, &input.lot_id)) {
                continue;
            }
            if let Some((parent, cursor)) = material_splits.get(&input.lot_id) {
                edges.push(Edge {
                    edge_id: format!("{}-material-split-{cursor}", report.id),
                    kind: "material_split",
                    work_order,
                    source: Node {
                        kind: "material_lot",
                        id: parent.as_str().into(),
                    },
                    target: Node {
                        kind: "material_lot",
                        id: input.lot_id.as_str().into(),
                    },
                    status: status(report.reversed),
                    quantity: None,
                    unit: None,
                    reservation_id: None,
                    operation: None,
                    work_center: None,
                    process_record: None,
                });
            }
            edges.push(Edge {
                edge_id: format!("{}-input-{index}", report.id),
                kind: "material_input",
                work_order,
                source: Node {
                    kind: "material_lot",
                    id: input.lot_id.as_str().into(),
                },
                target: wip(),
                status: status(report.reversed),
                quantity: Some(&input.quantity),
                unit: Some(&input.unit),
                reservation_id: Some(&input.reservation_id),
                operation: None,
                work_center: None,
                process_record: None,
            });
        }
        if let Some(target) = &report.consolidated_into {
            edges.push(Edge {
                edge_id: format!("{}-consolidate-{target}", report.id),
                kind: "wip_merge",
                work_order,
                source: wip(),
                target: Node {
                    kind: "wip_lot",
                    id: target.as_str().into(),
                },
                status: status(report.reversed),
                quantity: None,
                unit: None,
                reservation_id: None,
                operation: None,
                work_center: None,
                process_record: None,
            });
        }
        for (index, number) in report
            .serial_numbers
            .iter()
            .filter(|_| !consolidated)
            .enumerate()
        {
            if serial.is_some_and(|serial| number != serial) {
                continue;
            }
            edges.push(Edge {
                edge_id: format!("{}-serial-{index}", report.id),
                kind: "serial_output",
                work_order,
                source: wip(),
                target: Node {
                    kind: "serial",
                    id: number.as_str().into(),
                },
                status: status(report.reversed),
                quantity: None,
                unit: None,
                reservation_id: None,
                operation: None,
                work_center: None,
                process_record: None,
            });
            for output in receipts
                .iter()
                .filter(|output| output.order == report.order && output.serials.contains(number))
            {
                edges.push(Edge {
                    edge_id: format!("output-{}-{}-{index}", report.id, output.cursor),
                    kind: "finished_output_receipt",
                    work_order,
                    source: Node {
                        kind: "serial",
                        id: number.as_str().into(),
                    },
                    target: Node {
                        kind: "stock_lot",
                        id: output.lot.as_str().into(),
                    },
                    status: status(report.reversed || output.reversed),
                    quantity: None,
                    unit: None,
                    reservation_id: None,
                    operation: None,
                    work_center: None,
                    process_record: None,
                });
            }
        }
        for split in &report.wip_partitions {
            for child in &split.children {
                if serial.is_some_and(|serial| {
                    !child.serial_numbers.iter().any(|number| number == serial)
                }) || (consolidated
                    && lot.is_some()
                    && !child
                        .serial_numbers
                        .iter()
                        .any(|number| source_serials.contains(number)))
                {
                    continue;
                }
                let child_node = || Node {
                    kind: "wip_lot",
                    id: child.id.as_str().into(),
                };
                edges.push(Edge {
                    edge_id: format!("{}-split", child.id),
                    kind: "wip_split",
                    work_order,
                    source: wip(),
                    target: child_node(),
                    status: status(report.reversed),
                    quantity: None,
                    unit: None,
                    reservation_id: None,
                    operation: None,
                    work_center: None,
                    process_record: None,
                });
                if operation_detail && let Some(coating) = &report.coating {
                    edges.push(Edge {
                        edge_id: format!("{}-coating-output", child.id),
                        kind: "coating_output",
                        work_order,
                        source: coating_node(coating),
                        target: child_node(),
                        status: status(report.reversed || split.merged_cursor.is_some()),
                        quantity: None,
                        unit: None,
                        reservation_id: None,
                        operation: Some("coating"),
                        work_center: Some(&coating.work_center),
                        process_record: Some(&coating.process_record),
                    });
                }
                if split.merged_cursor.is_some() {
                    edges.push(Edge {
                        edge_id: format!("{}-merge", child.id),
                        kind: "wip_merge",
                        work_order,
                        source: child_node(),
                        target: wip(),
                        status: status(report.reversed),
                        quantity: None,
                        unit: None,
                        reservation_id: None,
                        operation: None,
                        work_center: None,
                        process_record: None,
                    });
                }
                if action != Action::TraceSerialBackward || operation_detail {
                    for consumer in consumers.get(child.id.as_str()).into_iter().flatten() {
                        edges.extend(append_assembly_outputs(consumer, &child.serial_numbers));
                        edges.push(Edge {
                            edge_id: format!("{}-use-{}", child.id, consumer.id),
                            kind: "wip_consumption",
                            work_order,
                            source: child_node(),
                            target: Node {
                                kind: "operation_report",
                                id: consumer.id.as_str().into(),
                            },
                            status: status(report.reversed || consumer.reversed),
                            quantity: None,
                            unit: None,
                            reservation_id: None,
                            operation: None,
                            work_center: None,
                            process_record: None,
                        });
                    }
                }
            }
        }
        if action != Action::TraceSerialBackward || operation_detail {
            for consumer in consumers.get(report.id.as_str()).into_iter().flatten() {
                edges.extend(append_assembly_outputs(consumer, &report.serial_numbers));
                edges.push(Edge {
                    edge_id: format!("{}-use-{}", report.id, consumer.id),
                    kind: "wip_consumption",
                    work_order,
                    source: wip(),
                    target: Node {
                        kind: "operation_report",
                        id: consumer.id.as_str().into(),
                    },
                    status: status(report.reversed || consumer.reversed),
                    quantity: None,
                    unit: None,
                    reservation_id: None,
                    operation: None,
                    work_center: None,
                    process_record: None,
                });
            }
        }
    }
    edges.retain(|edge| after.is_none_or(|after| edge.edge_id.as_str() > after));
    edges.sort_unstable_by(|left, right| left.edge_id.cmp(&right.edge_id));
    if operation_detail {
        edges.dedup_by(|left, right| left.edge_id == right.edge_id);
    }
    let next = (edges.len() > limit).then(|| edges[limit - 1].edge_id.clone());
    edges.truncate(limit);
    Ok(json!({"ok":true,"run_id":run,"edges":edges,"next_after_edge_id":next}))
}
