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
    (
        Action::CheckHarnessCrimpQualification,
        "check_harness_crimp_qualification",
        "检查线束端点的端子批次与压接工装资格",
        "quality",
    ),
];

const SEGMENTS: [(&str, u64); 2] = [("POWER", 400), ("SIGNAL", 250)];
const TRIM_PER_SEGMENT_MM: u64 = 5;
const SERIALS: [&str; 3] = ["WIRE-01", "WIRE-02", "WIRE-03"];

pub(super) fn spec(action: Action) -> Spec {
    let &(_, name, description, domain) = TOOLS.iter().find(|tool| tool.0 == action).unwrap();
    if action == Action::CheckHarnessCrimpQualification {
        return qualification_spec(name, description, domain);
    }
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

fn qualification_spec(name: &'static str, description: &'static str, domain: &'static str) -> Spec {
    let end = json!({"type":"string","enum":["A","B"]});
    Spec {
        name,
        description,
        domain,
        effect: "read",
        input: object(
            &[
                "serial_number",
                "segment_id",
                "end",
                "terminal_lot",
                "tooling_id",
            ],
            json!({"serial_number":text(),"segment_id":text(),"end":end,"terminal_lot":text(),"tooling_id":text()}),
        ),
        output: object(
            &["ok", "qualified", "qualification"],
            json!({
                "ok":{"const":true},"qualified":{"const":true},"qualification":object(
                    &["serial_number","segment_id","end","cut_batch_id","terminal_lot","tooling_id",
                        "terminal_qualification","tooling_qualification","process_revision"],
                    json!({"serial_number":text(),"segment_id":text(),"end":end,"cut_batch_id":text(),
                        "terminal_lot":text(),"tooling_id":text(),"terminal_qualification":text(),
                        "tooling_qualification":text(),"process_revision":text()}))
            }),
        ),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: "none",
        errors: &[
            "SERIAL_NOT_FOUND",
            "ENDPOINT_NOT_FOUND",
            "SERIAL_NOT_CUT",
            "TERMINAL_LOT_NOT_QUALIFIED",
            "TOOLING_NOT_QUALIFIED",
            "TOOLING_CALIBRATION_EXPIRED",
        ],
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
        if action == Action::CheckHarnessCrimpQualification {
            return match self.crimp_qualification(args) {
                Ok(qualification) => {
                    json!({"ok":true,"qualified":true,"qualification":qualification})
                }
                Err(code) => error(code),
            };
        }
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
    fn crimp_qualification(&self, args: &Map<String, Value>) -> Result<Value, &'static str> {
        let serial = args["serial_number"].as_str().unwrap();
        let segment = args["segment_id"].as_str().unwrap();
        let end = args["end"].as_str().unwrap();
        let terminal = args["terminal_lot"].as_str().unwrap();
        let tooling = args["tooling_id"].as_str().unwrap();
        if !SERIALS.contains(&serial) {
            return Err("SERIAL_NOT_FOUND");
        }
        if !SEGMENTS.iter().any(|(id, _)| *id == segment) {
            return Err("ENDPOINT_NOT_FOUND");
        }
        let batch = self
            .batches
            .values()
            .find(|batch| batch.segments.contains_key(serial))
            .ok_or("SERIAL_NOT_CUT")?;
        let terminal_qualification = match (segment, end, terminal) {
            ("POWER", _, "TERM-PWR-1") => "TERM-PWR@1",
            ("POWER", "B", "TERM-PWR-ALT") => "TERM-PWR-B-ALT@1",
            ("SIGNAL", _, "TERM-SIG-1") => "TERM-SIG@1",
            _ => return Err("TERMINAL_LOT_NOT_QUALIFIED"),
        };
        let tooling_qualification = match (segment, tooling) {
            ("POWER", "TOOL-PWR-1") => "TOOL-PWR@1",
            ("SIGNAL", "TOOL-SIG-1") => "TOOL-SIG@1",
            ("POWER", "TOOL-PWR-EXPIRED") => return Err("TOOLING_CALIBRATION_EXPIRED"),
            _ => return Err("TOOLING_NOT_QUALIFIED"),
        };
        Ok(
            json!({"serial_number":serial,"segment_id":segment,"end":end,"cut_batch_id":batch.batch_id,
            "terminal_lot":terminal,"tooling_id":tooling,"terminal_qualification":terminal_qualification,
            "tooling_qualification":tooling_qualification,"process_revision":"HARNESS-CRIMP-1"}),
        )
    }
}
