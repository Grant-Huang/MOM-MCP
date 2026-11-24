use std::collections::BTreeMap;

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
    id: &'a str,
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
        json!({"kind":{"enum":["material_lot","wip_lot","serial","operation_report"]},"id":id_schema()}),
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
        "edge_id":id_schema(),"kind":{"enum":["material_input","serial_output","wip_consumption"]},
        "work_order":id_schema(),"source":node,"target":node,"status":{"enum":["active","reversed"]},
        "reservation_id":id_schema(),"quantity":quantity_schema(),"unit":text()}),
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

pub(crate) fn read(
    run: &str,
    action: Action,
    reports: &[Report],
    args: &Map<String, Value>,
) -> Value {
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
        return json!({"ok":false,"code":"INVALID_ARGUMENT"});
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

    let mut edges = Vec::new();
    for report in reports
        .iter()
        .filter(|report| report.step == 0 && order.is_none_or(|order| report.order == order))
    {
        if lot.is_some_and(|lot| {
            !report
                .material_inputs
                .iter()
                .any(|input| input.lot_id == lot)
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
            id: &report.id,
        };
        for (index, input) in report.material_inputs.iter().enumerate() {
            if lot.is_some_and(|lot| input.lot_id != lot) {
                continue;
            }
            edges.push(Edge {
                edge_id: format!("{}-input-{index}", report.id),
                kind: "material_input",
                work_order,
                source: Node {
                    kind: "material_lot",
                    id: &input.lot_id,
                },
                target: wip(),
                status: status(report.reversed),
                quantity: Some(&input.quantity),
                unit: Some(&input.unit),
                reservation_id: Some(&input.reservation_id),
            });
        }
        for (index, number) in report.serial_numbers.iter().enumerate() {
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
                    id: number,
                },
                status: status(report.reversed),
                quantity: None,
                unit: None,
                reservation_id: None,
            });
        }
        if action != Action::TraceSerialBackward {
            for consumer in consumers.get(report.id.as_str()).into_iter().flatten() {
                edges.push(Edge {
                    edge_id: format!("{}-use-{}", report.id, consumer.id),
                    kind: "wip_consumption",
                    work_order,
                    source: wip(),
                    target: Node {
                        kind: "operation_report",
                        id: &consumer.id,
                    },
                    status: status(report.reversed || consumer.reversed),
                    quantity: None,
                    unit: None,
                    reservation_id: None,
                });
            }
        }
    }
    edges.retain(|edge| after.is_none_or(|after| edge.edge_id.as_str() > after));
    edges.sort_unstable_by(|left, right| left.edge_id.cmp(&right.edge_id));
    let next = (edges.len() > limit).then(|| edges[limit - 1].edge_id.clone());
    edges.truncate(limit);
    json!({"ok":true,"run_id":run,"edges":edges,"next_after_edge_id":next})
}
