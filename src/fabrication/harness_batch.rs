use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{Action, Map, Spec, Value, error, json, object, text};

pub(super) const TOOLS: &[(Action, &str, &str, &str)] = &[
    (
        Action::CutHarnessBatch,
        "cut_harness_batch",
        "按原线批次批量裁切线束线段",
        "production",
    ),
    (
        Action::GetHarnessCutBatch,
        "get_harness_cut_batch",
        "查询裁线批次及序列号投入",
        "production",
    ),
];

const SEGMENTS: [(&str, u64); 2] = [("POWER", 400), ("SIGNAL", 250)];
const TRIM_PER_SEGMENT_MM: u64 = 5;
const SERIALS: [&str; 3] = ["WIRE-01", "WIRE-02", "WIRE-03"];

pub(super) fn spec(action: Action) -> Spec {
    let &(_, name, description, domain) = TOOLS.iter().find(|tool| tool.0 == action).unwrap();
    let read = action == Action::GetHarnessCutBatch;
    let length = json!({"type":"integer","minimum":0});
    let input = if read {
        object(&["batch_id"], json!({"batch_id":text()}))
    } else {
        object(
            &["batch_id", "wire_lot", "serial_numbers", "idempotency_key"],
            json!({
                "batch_id":text(),"wire_lot":text(),"idempotency_key":text(),
                "serial_numbers":{"type":"array","minItems":1,"maxItems":20,"uniqueItems":true,"items":text()}
            }),
        )
    };
    let segment = object(
        &["segment_id", "length_mm"],
        json!({"segment_id":text(),"length_mm":length}),
    );
    let batch = object(
        &[
            "batch_id",
            "work_order",
            "wire_lot",
            "process_revision",
            "segments",
            "input_mm",
            "product_mm",
            "trim_mm",
            "remaining_mm",
        ],
        json!({"batch_id":text(),"work_order":text(),"wire_lot":text(),"process_revision":text(),
            "segments":{"type":"object","additionalProperties":{"type":"array","items":segment}},
            "input_mm":length,"product_mm":length,"trim_mm":length,"remaining_mm":length}),
    );
    Spec {
        name,
        description,
        domain,
        input,
        effect: if read { "read" } else { "write" },
        output: object(
            &["ok", "batch"],
            json!({"ok":{"const":true},"batch":batch,"cursor":length}),
        ),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: if read {
            "none"
        } else {
            "record_wire_cut_batch"
        },
        errors: if read {
            &["CUT_BATCH_NOT_FOUND"]
        } else {
            &[
                "DUPLICATE_CUT_BATCH",
                "SERIAL_NOT_FOUND",
                "DUPLICATE_SERIAL",
                "SERIAL_ALREADY_CUT",
                "WIRE_LOT_NOT_FOUND",
                "WIRE_LOT_NOT_QUALIFIED",
                "INSUFFICIENT_WIRE_LENGTH",
                "IDEMPOTENCY_CONFLICT",
            ]
        },
        visibility: "run",
    }
}

#[derive(Serialize, Deserialize)]
struct Segment {
    segment_id: String,
    length_mm: u64,
}

#[derive(Serialize, Deserialize)]
struct CutBatch {
    batch_id: String,
    work_order: String,
    wire_lot: String,
    process_revision: String,
    segments: BTreeMap<String, Vec<Segment>>,
    input_mm: u64,
    product_mm: u64,
    trim_mm: u64,
    remaining_mm: u64,
}

#[derive(Serialize, Deserialize)]
struct WireLot {
    remaining_mm: u64,
    qualified: bool,
}

#[derive(Serialize, Deserialize)]
pub(super) struct HarnessBatches {
    lots: BTreeMap<String, WireLot>,
    batches: BTreeMap<String, CutBatch>,
}

impl Default for HarnessBatches {
    fn default() -> Self {
        let lots = [
            ("WIRE-CU-1", 1500, true),
            ("WIRE-CU-2", 800, true),
            ("WIRE-OTHER", 2000, false),
        ]
        .into_iter()
        .map(|(id, remaining_mm, qualified)| {
            (
                id.into(),
                WireLot {
                    remaining_mm,
                    qualified,
                },
            )
        })
        .collect();
        Self {
            lots,
            batches: BTreeMap::new(),
        }
    }
}

impl HarnessBatches {
    pub(super) fn apply(
        &mut self,
        action: Action,
        args: &Map<String, Value>,
        work_order: &str,
    ) -> Value {
        let id = args["batch_id"].as_str().unwrap();
        if action == Action::GetHarnessCutBatch {
            return self.batches.get(id).map_or_else(
                || error("CUT_BATCH_NOT_FOUND"),
                |batch| json!({"ok":true,"batch":batch}),
            );
        }
        if self.batches.contains_key(id) {
            return error("DUPLICATE_CUT_BATCH");
        }
        let mut serials = BTreeSet::new();
        for serial in args["serial_numbers"].as_array().unwrap() {
            let serial = serial.as_str().unwrap();
            if !SERIALS.contains(&serial) {
                return error("SERIAL_NOT_FOUND");
            }
            if !serials.insert(serial) {
                return error("DUPLICATE_SERIAL");
            }
            if self
                .batches
                .values()
                .any(|batch| batch.segments.contains_key(serial))
            {
                return error("SERIAL_ALREADY_CUT");
            }
        }
        let lot_id = args["wire_lot"].as_str().unwrap();
        let Some(lot) = self.lots.get_mut(lot_id) else {
            return error("WIRE_LOT_NOT_FOUND");
        };
        if !lot.qualified {
            return error("WIRE_LOT_NOT_QUALIFIED");
        }
        let count = serials.len() as u64;
        let product_mm = count * SEGMENTS.iter().map(|(_, length)| length).sum::<u64>();
        let trim_mm = count * SEGMENTS.len() as u64 * TRIM_PER_SEGMENT_MM;
        let input_mm = product_mm + trim_mm;
        let Some(remaining_mm) = lot.remaining_mm.checked_sub(input_mm) else {
            return error("INSUFFICIENT_WIRE_LENGTH");
        };
        let segments = serials
            .into_iter()
            .map(|serial| {
                (
                    serial.into(),
                    SEGMENTS
                        .iter()
                        .map(|(id, length_mm)| Segment {
                            segment_id: (*id).into(),
                            length_mm: *length_mm,
                        })
                        .collect(),
                )
            })
            .collect();
        let batch = CutBatch {
            batch_id: id.into(),
            work_order: work_order.into(),
            wire_lot: lot_id.into(),
            process_revision: "HARNESS-CUT-1".into(),
            segments,
            input_mm,
            product_mm,
            trim_mm,
            remaining_mm,
        };
        let result = json!({"ok":true,"batch":batch});
        lot.remaining_mm = remaining_mm;
        self.batches.insert(id.into(), batch);
        result
    }
}
