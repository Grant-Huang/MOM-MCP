use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{Action, Event, Map, Spec, Value, error, json, object, text};

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
    (
        Action::CrimpHarnessEndpoint,
        "crimp_harness_endpoint",
        "校验资格、扣用端子并记录端点压接",
        "production",
    ),
    (
        Action::GetHarnessCrimpStatus,
        "get_harness_crimp_status",
        "查询线束已压接端点及工序进度",
        "production",
    ),
    (
        Action::ApplyHarnessSampleHold,
        "apply_harness_sample_hold",
        "按抽检结果隔离端子批次及实际受影响端点",
        "quality",
    ),
    (
        Action::ReworkHarnessEndpoint,
        "rework_harness_endpoint",
        "更换已隔离端点的端子并登记返工版本",
        "quality",
    ),
    (
        Action::RecordHarnessContinuityTest,
        "record_harness_continuity_test",
        "登记当前线束版本的导通测试结果",
        "quality",
    ),
    (
        Action::ReleaseHarnessSerial,
        "release_harness_serial",
        "放行当前版本导通测试合格的线束",
        "quality",
    ),
];

const SEGMENTS: [(&str, u64); 2] = [("POWER", 400), ("SIGNAL", 250)];
const TRIM_PER_SEGMENT_MM: u64 = 5;
const SERIALS: [&str; 3] = ["WIRE-01", "WIRE-02", "WIRE-03"];

pub(super) fn spec(action: Action) -> Spec {
    let &(_, name, description, domain) = TOOLS.iter().find(|tool| tool.0 == action).unwrap();
    if matches!(
        action,
        Action::ReworkHarnessEndpoint
            | Action::RecordHarnessContinuityTest
            | Action::ReleaseHarnessSerial
    ) {
        return disposition_spec(action, name, description, domain);
    }
    if action == Action::ApplyHarnessSampleHold {
        let cursor = json!({"type":"integer","minimum":1});
        return Spec {
            name,
            description,
            domain,
            effect: "write",
            input: object(
                &["event_cursor", "idempotency_key"],
                json!({"event_cursor":cursor,"idempotency_key":text()}),
            ),
            output: object(
                &[
                    "ok",
                    "event_cursor",
                    "terminal_lot",
                    "affected_endpoints",
                    "cursor",
                ],
                json!({
                    "ok":{"const":true},"event_cursor":cursor,"terminal_lot":text(),
                    "affected_endpoints":{"type":"array","items":endpoint_schema()},"cursor":cursor
                }),
            ),
            preconditions: &["run_open", "tool_enabled_in_scenario"],
            side_effect: "isolate_terminal_lot_and_endpoints",
            errors: &[
                "SAMPLE_NOT_DELIVERED",
                "TERMINAL_LOT_ALREADY_HELD",
                "IDEMPOTENCY_CONFLICT",
            ],
            visibility: "run",
        };
    }
    if matches!(
        action,
        Action::CheckHarnessCrimpQualification
            | Action::CrimpHarnessEndpoint
            | Action::GetHarnessCrimpStatus
    ) {
        return crimp_spec(action, name, description, domain);
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

fn disposition_spec(
    action: Action,
    name: &'static str,
    description: &'static str,
    domain: &'static str,
) -> Spec {
    let mut fields = json!({"serial_number":text(),"idempotency_key":text()});
    let mut required = vec!["serial_number", "idempotency_key"];
    if action == Action::ReworkHarnessEndpoint {
        required.extend(["segment_id", "end", "terminal_lot", "tooling_id", "reason"]);
        fields["segment_id"] = text();
        fields["end"] = json!({"type":"string","enum":["A","B"]});
        fields["terminal_lot"] = text();
        fields["tooling_id"] = text();
        fields["reason"] = text();
    }
    if action == Action::RecordHarnessContinuityTest {
        required.extend(["result", "evidence", "revision"]);
        fields["revision"] = json!({"type":"integer","minimum":1});
        fields["result"] = json!({"type":"string","enum":["pass","fail"]});
        fields["evidence"] = text();
    }
    Spec {
        name,
        description,
        domain,
        effect: "write",
        input: object(&required, fields),
        output: json!({"type":"object","required":["ok","serial_number","revision","cursor"],"properties":{
            "ok":{"const":true},"serial_number":text(),"revision":{"type":"integer","minimum":1},
            "cursor":{"type":"integer","minimum":1},"rework":{"type":"object"},"test":{"type":"object"},"released":{"type":"boolean"}
        }}),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: "update_harness_disposition",
        errors: &[
            "SERIAL_NOT_FOUND",
            "SERIAL_NOT_CUT",
            "ENDPOINT_NOT_FOUND",
            "ENDPOINT_NOT_HELD",
            "ENDPOINTS_INCOMPLETE",
            "TERMINAL_QUALITY_HOLD",
            "TERMINAL_LOT_NOT_QUALIFIED",
            "TERMINAL_STOCK_EXHAUSTED",
            "TOOLING_NOT_QUALIFIED",
            "TOOLING_CALIBRATION_EXPIRED",
            "TEST_PASS_REQUIRED",
            "TEST_REVISION_MISMATCH",
            "SERIAL_ALREADY_RELEASED",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

fn endpoint_schema() -> Value {
    object(
        &["serial_number", "segment_id", "end", "terminal_lot"],
        json!({
            "serial_number":text(),"segment_id":text(),"end":{"type":"string","enum":["A","B"]},"terminal_lot":text()
        }),
    )
}

fn crimp_spec(
    action: Action,
    name: &'static str,
    description: &'static str,
    domain: &'static str,
) -> Spec {
    let end = json!({"type":"string","enum":["A","B"]});
    let count = json!({"type":"integer","minimum":0});
    let qualification = object(
        &[
            "serial_number",
            "segment_id",
            "end",
            "cut_batch_id",
            "terminal_lot",
            "tooling_id",
            "terminal_qualification",
            "tooling_qualification",
            "process_revision",
        ],
        json!({"serial_number":text(),"segment_id":text(),"end":end,"cut_batch_id":text(),
            "terminal_lot":text(),"tooling_id":text(),"terminal_qualification":text(),
            "tooling_qualification":text(),"process_revision":text()}),
    );
    let mut fields = json!({"serial_number":text(),"segment_id":text(),"end":end,"terminal_lot":text(),"tooling_id":text()});
    let mut required = vec![
        "serial_number",
        "segment_id",
        "end",
        "terminal_lot",
        "tooling_id",
    ];
    let write = action == Action::CrimpHarnessEndpoint;
    let (output, errors): (_, &'static [&'static str]) = match action {
        Action::GetHarnessCrimpStatus => {
            fields = json!({"serial_number":text()});
            required.truncate(1);
            (
                object(
                    &[
                        "ok",
                        "serial_number",
                        "complete",
                        "required_endpoints",
                        "crimps",
                    ],
                    json!({
                        "ok":{"const":true},"serial_number":text(),"complete":{"type":"boolean"},
                        "required_endpoints":count,"crimps":{"type":"array","items":qualification},
                        "held_endpoints":{"type":"array","items":endpoint_schema()},
                        "disposition":{"type":"object"}
                    }),
                ),
                &["SERIAL_NOT_FOUND", "SERIAL_NOT_CUT"],
            )
        }
        Action::CrimpHarnessEndpoint => {
            fields["idempotency_key"] = text();
            required.push("idempotency_key");
            (
                object(
                    &["ok", "crimp", "remaining_terminals", "cursor"],
                    json!({
                        "ok":{"const":true},"crimp":qualification,"remaining_terminals":count,"cursor":count
                    }),
                ),
                &[
                    "SERIAL_NOT_FOUND",
                    "ENDPOINT_NOT_FOUND",
                    "SERIAL_NOT_CUT",
                    "TERMINAL_LOT_NOT_QUALIFIED",
                    "TOOLING_NOT_QUALIFIED",
                    "TOOLING_CALIBRATION_EXPIRED",
                    "ENDPOINT_ALREADY_CRIMPED",
                    "TERMINAL_STOCK_EXHAUSTED",
                    "TERMINAL_QUALITY_HOLD",
                    "IDEMPOTENCY_CONFLICT",
                ],
            )
        }
        _ => (
            object(
                &["ok", "qualified", "qualification"],
                json!({
                    "ok":{"const":true},"qualified":{"const":true},"qualification":qualification
                }),
            ),
            &[
                "SERIAL_NOT_FOUND",
                "ENDPOINT_NOT_FOUND",
                "SERIAL_NOT_CUT",
                "TERMINAL_LOT_NOT_QUALIFIED",
                "TOOLING_NOT_QUALIFIED",
                "TOOLING_CALIBRATION_EXPIRED",
                "TERMINAL_QUALITY_HOLD",
            ],
        ),
    };
    Spec {
        name,
        description,
        domain,
        effect: if write { "write" } else { "read" },
        input: object(&required, fields),
        output,
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: if write {
            "record_harness_crimp"
        } else {
            "none"
        },
        errors,
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
struct Disposition {
    revision: u64,
    reworks: Vec<Value>,
    tests: Vec<Value>,
    released: bool,
}

impl Default for Disposition {
    fn default() -> Self {
        Self {
            revision: 1,
            reworks: Vec::new(),
            tests: Vec::new(),
            released: false,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct HarnessBatches {
    lots: BTreeMap<String, WireLot>,
    batches: BTreeMap<String, CutBatch>,
    #[serde(default)]
    crimps: BTreeMap<String, Vec<Value>>,
    #[serde(default = "terminal_stock")]
    terminals: BTreeMap<String, u64>,
    #[serde(default)]
    sample_holds: BTreeMap<String, usize>,
    #[serde(default)]
    dispositions: BTreeMap<String, Disposition>,
}

fn terminal_stock() -> BTreeMap<String, u64> {
    [
        ("TERM-PWR-1", 6),
        ("TERM-PWR-ALT", 1),
        ("TERM-PWR-2", 3),
        ("TERM-SIG-1", 6),
    ]
    .into_iter()
    .map(|(id, quantity)| (id.into(), quantity))
    .collect()
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
            crimps: BTreeMap::new(),
            terminals: terminal_stock(),
            sample_holds: BTreeMap::new(),
            dispositions: BTreeMap::new(),
        }
    }
}

impl HarnessBatches {
    pub(super) fn apply(
        &mut self,
        action: Action,
        args: &Map<String, Value>,
        work_order: &str,
        events: &[Event],
    ) -> Value {
        if matches!(
            action,
            Action::ReworkHarnessEndpoint
                | Action::RecordHarnessContinuityTest
                | Action::ReleaseHarnessSerial
        ) {
            return self.disposition(action, args);
        }
        if action == Action::ApplyHarnessSampleHold {
            return self.apply_sample_hold(args["event_cursor"].as_u64().unwrap(), events);
        }
        if action == Action::CheckHarnessCrimpQualification {
            return match self.crimp_qualification(args, false) {
                Ok(qualification) => {
                    json!({"ok":true,"qualified":true,"qualification":qualification})
                }
                Err(code) => error(code),
            };
        }
        if action == Action::GetHarnessCrimpStatus {
            return self.crimp_status(args["serial_number"].as_str().unwrap());
        }
        if action == Action::CrimpHarnessEndpoint {
            return self.crimp_endpoint(args);
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
    fn apply_sample_hold(&mut self, cursor: u64, events: &[Event]) -> Value {
        let Some(event) = events.iter().find(|event| {
            event.cursor as u64 == cursor && event.kind == "harness_terminal_sample_failed"
        }) else {
            return error("SAMPLE_NOT_DELIVERED");
        };
        let lot = event.payload["terminal_lot"]
            .as_str()
            .expect("terminal sample lot");
        if self.sample_holds.contains_key(lot) {
            return error("TERMINAL_LOT_ALREADY_HELD");
        }
        let affected: Vec<_> = self
            .crimps
            .values()
            .flatten()
            .filter(|crimp| crimp["terminal_lot"] == lot)
            .map(Self::endpoint)
            .collect();
        self.sample_holds.insert(lot.into(), event.cursor);
        json!({"ok":true,"event_cursor":cursor,"terminal_lot":lot,"affected_endpoints":affected})
    }

    fn endpoint(crimp: &Value) -> Value {
        json!({"serial_number":crimp["serial_number"],"segment_id":crimp["segment_id"],
            "end":crimp["end"],"terminal_lot":crimp["terminal_lot"]})
    }

    fn disposition(&mut self, action: Action, args: &Map<String, Value>) -> Value {
        let serial = args["serial_number"].as_str().unwrap();
        if self
            .dispositions
            .get(serial)
            .is_some_and(|entry| entry.released)
        {
            return error("SERIAL_ALREADY_RELEASED");
        }
        let status = self.crimp_status(serial);
        if status["ok"] != true {
            return status;
        }
        if action == Action::ReworkHarnessEndpoint {
            let Some(index) = self.crimps.get(serial).and_then(|records| {
                records.iter().position(|crimp| {
                    crimp["segment_id"] == args["segment_id"]
                        && crimp["end"] == args["end"]
                        && crimp["terminal_lot"]
                            .as_str()
                            .is_some_and(|lot| self.sample_holds.contains_key(lot))
                })
            }) else {
                return error("ENDPOINT_NOT_HELD");
            };
            let mut replacement = match self.crimp_qualification(args, true) {
                Ok(crimp) => crimp,
                Err(code) => return error(code),
            };
            let terminal = replacement["terminal_lot"].as_str().unwrap();
            let Some(remaining) = self
                .terminals
                .get_mut(terminal)
                .filter(|quantity| **quantity > 0)
            else {
                return error("TERMINAL_STOCK_EXHAUSTED");
            };
            *remaining -= 1;
            replacement["process_revision"] = json!("HARNESS-REWORK-1");
            let previous = std::mem::replace(
                &mut self.crimps.get_mut(serial).unwrap()[index],
                replacement.clone(),
            );
            let entry = self.dispositions.entry(serial.into()).or_default();
            entry.revision += 1;
            let rework = json!({"revision":entry.revision,"previous":previous,"replacement":replacement,"reason":args["reason"]});
            entry.reworks.push(rework.clone());
            return json!({"ok":true,"serial_number":serial,"revision":entry.revision,"rework":rework});
        }
        if status["complete"] != true {
            return error("ENDPOINTS_INCOMPLETE");
        }
        if status["held_endpoints"]
            .as_array()
            .is_some_and(|endpoints| !endpoints.is_empty())
        {
            return error("TERMINAL_QUALITY_HOLD");
        }
        if action == Action::ReleaseHarnessSerial {
            let Some(entry) = self.dispositions.get_mut(serial).filter(|entry| {
                entry.tests.last().is_some_and(|test| {
                    test["revision"] == entry.revision && test["result"] == "pass"
                })
            }) else {
                return error("TEST_PASS_REQUIRED");
            };
            entry.released = true;
            return json!({"ok":true,"serial_number":serial,"revision":entry.revision,"released":true});
        }
        let entry = self.dispositions.entry(serial.into()).or_default();
        if args["revision"].as_u64() != Some(entry.revision) {
            return error("TEST_REVISION_MISMATCH");
        }
        let test =
            json!({"revision":entry.revision,"result":args["result"],"evidence":args["evidence"]});
        entry.tests.push(test.clone());
        json!({"ok":true,"serial_number":serial,"revision":entry.revision,"test":test})
    }

    fn crimp_status(&self, serial: &str) -> Value {
        if !SERIALS.contains(&serial) {
            return error("SERIAL_NOT_FOUND");
        }
        let Some(segments) = self
            .batches
            .values()
            .find_map(|batch| batch.segments.get(serial))
        else {
            return error("SERIAL_NOT_CUT");
        };
        let required_endpoints = segments.len() * 2;
        let crimps = self.crimps.get(serial).map_or(&[][..], Vec::as_slice);
        let mut result = json!({"ok":true,"serial_number":serial,"complete":crimps.len()==required_endpoints,
            "required_endpoints":required_endpoints,"crimps":crimps});
        if !self.sample_holds.is_empty() {
            result["held_endpoints"] = json!(
                crimps
                    .iter()
                    .filter(|crimp| crimp["terminal_lot"]
                        .as_str()
                        .is_some_and(|lot| self.sample_holds.contains_key(lot)))
                    .map(Self::endpoint)
                    .collect::<Vec<_>>()
            );
        }
        if let Some(disposition) = self.dispositions.get(serial) {
            result["disposition"] = json!(disposition);
        }
        result
    }

    fn crimp_endpoint(&mut self, args: &Map<String, Value>) -> Value {
        let crimp = match self.crimp_qualification(args, false) {
            Ok(crimp) => crimp,
            Err(code) => return error(code),
        };
        let serial = crimp["serial_number"].as_str().unwrap();
        if self.crimps.get(serial).is_some_and(|records| {
            records.iter().any(|previous| {
                previous["segment_id"] == crimp["segment_id"] && previous["end"] == crimp["end"]
            })
        }) {
            return error("ENDPOINT_ALREADY_CRIMPED");
        }
        let terminal = crimp["terminal_lot"].as_str().unwrap();
        let Some(remaining) = self
            .terminals
            .get_mut(terminal)
            .filter(|quantity| **quantity > 0)
        else {
            return error("TERMINAL_STOCK_EXHAUSTED");
        };
        *remaining -= 1;
        let result = json!({"ok":true,"crimp":crimp,"remaining_terminals":*remaining});
        self.crimps.entry(serial.into()).or_default().push(crimp);
        result
    }

    fn crimp_qualification(
        &self,
        args: &Map<String, Value>,
        rework: bool,
    ) -> Result<Value, &'static str> {
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
            ("POWER", _, "TERM-PWR-2") if rework => "TERM-PWR-REWORK@1",
            ("POWER", _, "TERM-PWR-1") => "TERM-PWR@1",
            ("POWER", "B", "TERM-PWR-ALT") => "TERM-PWR-B-ALT@1",
            ("SIGNAL", _, "TERM-SIG-1") => "TERM-SIG@1",
            _ => return Err("TERMINAL_LOT_NOT_QUALIFIED"),
        };
        if self.sample_holds.contains_key(terminal) {
            return Err("TERMINAL_QUALITY_HOLD");
        }
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
