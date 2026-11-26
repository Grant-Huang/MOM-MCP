mod layout;

use std::collections::{BTreeMap, BTreeSet};

use super::{
    Action, Event, Map, PART_AREA, SCRAP_AREA, Setup, Sheet, Spec, Stock, Value, error,
    event_matching, json, object, text,
};
use serde::{Deserialize, Serialize};

pub(super) const TOOLS: &[(Action, &str, &str, &str)] = &[
    (
        Action::CreateSheetNest,
        "create_sheet_nest",
        "创建多工单套料并锁定板材",
        "production",
    ),
    (
        Action::GetSheetNest,
        "get_sheet_nest",
        "查询套料、工单分摊与面积台账",
        "production",
    ),
    (
        Action::CutSheetNest,
        "cut_sheet_nest",
        "执行套料切割并登记工单用料",
        "production",
    ),
    (
        Action::CancelSheetNest,
        "cancel_sheet_nest",
        "取消待切套料并释放板材",
        "production",
    ),
    (
        Action::ReturnSheetRemnant,
        "return_sheet_remnant",
        "返库并登记可复用余料",
        "inventory",
    ),
    (
        Action::TraceSheetMaterial,
        "trace_sheet_material",
        "按板材、批次或工单查询实际切割谱系",
        "inventory",
    ),
];

pub(super) fn spec(action: Action) -> Spec {
    let &(_, name, description, domain) = TOOLS.iter().find(|tool| tool.0 == action).unwrap();
    let mut required = vec!["nest_id"];
    let mut fields = json!({"nest_id":text()});
    if action != Action::GetSheetNest {
        required.push("idempotency_key");
        fields["idempotency_key"] = text();
    }
    if action == Action::CreateSheetNest {
        required.extend(["stock", "work_orders"]);
        fields["stock"] = text();
        fields["work_orders"] =
            json!({"type":"array","minItems":1,"maxItems":20,"items":text(),"uniqueItems":true});
    }
    if action == Action::ReturnSheetRemnant {
        required.push("remnant_id");
        fields["remnant_id"] = text();
    }
    if action == Action::CutSheetNest {
        required.push("machine");
        fields["machine"] = text();
    }
    let nest = nest_schema();
    if action == Action::TraceSheetMaterial {
        let subject_type = json!({"type":"string","enum":["stock","lot","work_order"]});
        return Spec {
            name,
            description,
            domain,
            effect: "read",
            input: object(
                &["subject_type", "subject"],
                json!({"subject_type":subject_type,"subject":text()}),
            ),
            output: object(
                &["ok", "subject_type", "subject", "cuts"],
                json!({
                    "ok":{"const":true},"subject_type":subject_type,"subject":text(),"cuts":{"type":"array","items":nest}
                }),
            ),
            preconditions: &["run_open", "tool_enabled_in_scenario"],
            side_effect: "none",
            errors: &["TRACE_SUBJECT_NOT_FOUND"],
            visibility: "run",
        };
    }
    Spec {
        name,
        description,
        domain,
        effect: if action == Action::GetSheetNest {
            "read"
        } else {
            "write"
        },
        input: object(&required, fields),
        output: object(
            &["ok", "nest"],
            json!({
            "ok":{"const":true},"cursor":{"type":"integer"},"nest":nest}),
        ),
        preconditions: &["run_open", "tool_enabled_in_scenario"],
        side_effect: if action == Action::GetSheetNest {
            "none"
        } else {
            "update_sheet_nesting"
        },
        errors: &[
            "DUPLICATE_NEST",
            "DUPLICATE_WORK_ORDER",
            "WORK_ORDER_NOT_FOUND",
            "WORK_ORDER_ALLOCATED",
            "STOCK_NOT_FOUND",
            "STOCK_UNAVAILABLE",
            "STOCK_INELIGIBLE",
            "STOCK_RESERVED",
            "INSUFFICIENT_AREA",
            "STOCK_DIMENSIONS_INSUFFICIENT",
            "NEST_NOT_FOUND",
            "NEST_LAYOUT_UNAVAILABLE",
            "NEST_LAYOUT_INVALID",
            "NO_REUSABLE_REMNANT",
            "REMNANT_ALREADY_RETURNED",
            "STOCK_ID_EXISTS",
            "INVALID_STATE",
            "MACHINE_NOT_CAPABLE",
            "MACHINE_UNAVAILABLE",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

fn nest_schema() -> Value {
    let area = json!({"type":"integer","minimum":0});
    let dimensions = object(
        &["length_mm", "width_mm"],
        json!({
            "length_mm":{"type":"integer","minimum":1},"width_mm":{"type":"integer","minimum":1}
        }),
    );
    object(
        &[
            "nest_id",
            "stock",
            "lot",
            "status",
            "order_area_mm2",
            "input_mm2",
            "product_mm2",
            "scrap_mm2",
            "remnant_mm2",
        ],
        json!({"nest_id":text(),"stock":text(),"lot":text(),"status":{"enum":["planned","cut","cancelled"]},
        "order_area_mm2":{"type":"object","additionalProperties":area},
        "input_mm2":area,"product_mm2":area,"scrap_mm2":area,"remnant_mm2":area,"machine":text(),"remnant_dimensions":dimensions,"returned_remnant":text(),"parent_stock":text(),
        "placements":{"type":"object","additionalProperties":object(
            &["x_mm","y_mm","length_mm","width_mm"],
            json!({"x_mm":area,"y_mm":area,"length_mm":area,"width_mm":area}))},
        "dimensional_fit":object(&["stock_dimensions","size_grade","order_footprints","eligible_machines"], json!({
            "stock_dimensions":dimensions,"size_grade":{"enum":["standard","wide","strip"]},
            "order_footprints":{"type":"object","additionalProperties":dimensions},
            "eligible_machines":{"type":"array","items":text()}
        }))}),
    )
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Status {
    Planned,
    Cut,
    Cancelled,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct Dimensions {
    length_mm: u64,
    width_mm: u64,
}

impl Dimensions {
    const fn new(length_mm: u64, width_mm: u64) -> Self {
        Self {
            length_mm,
            width_mm,
        }
    }

    fn contains(self, footprint: Self) -> bool {
        self.length_mm >= footprint.length_mm && self.width_mm >= footprint.width_mm
    }
}

const MACHINE_ENVELOPES: [(&str, Dimensions); 2] = [
    ("LASER-A", Dimensions::new(50, 30)),
    ("LASER-B", Dimensions::new(80, 40)),
];

#[derive(Serialize, Deserialize)]
struct DimensionalFit {
    stock_dimensions: Dimensions,
    size_grade: String,
    order_footprints: BTreeMap<String, Dimensions>,
    eligible_machines: Vec<String>,
}

impl DimensionalFit {
    fn for_stock(
        stock: &str,
        orders: &BTreeMap<String, u64>,
        returned: Option<Dimensions>,
    ) -> Self {
        let (stock_dimensions, size_grade) = returned
            .map(|dimensions| (dimensions, "standard"))
            .unwrap_or_else(|| match stock {
                "SHEET-FULL" => (Dimensions::new(60, 30), "wide"),
                "REM-OK" => (Dimensions::new(50, 20), "standard"),
                "REM-SMALL" => (Dimensions::new(65, 10), "strip"),
                "REM-STRIP" => (Dimensions::new(75, 12), "strip"),
                _ => unreachable!("eligible stock has registered dimensions"),
            });
        let order_footprints = orders
            .keys()
            .map(|order| {
                let footprint = if order == "WO-SHEET-C" {
                    Dimensions::new(40, 15)
                } else {
                    Dimensions::new(30, 20)
                };
                (order.clone(), footprint)
            })
            .collect();
        let eligible_machines = MACHINE_ENVELOPES
            .iter()
            .filter(|(_, envelope)| envelope.contains(stock_dimensions))
            .map(|(machine, _)| (*machine).into())
            .collect();
        Self {
            stock_dimensions,
            size_grade: size_grade.into(),
            order_footprints,
            eligible_machines,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Nest {
    nest_id: String,
    stock: String,
    lot: String,
    status: Status,
    order_area_mm2: BTreeMap<String, u64>,
    input_mm2: u64,
    product_mm2: u64,
    scrap_mm2: u64,
    remnant_mm2: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    machine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dimensional_fit: Option<DimensionalFit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remnant_dimensions: Option<Dimensions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    returned_remnant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent_stock: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placements: Option<BTreeMap<String, layout::Placement>>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Nesting {
    inventory: BTreeMap<String, Stock>,
    nests: BTreeMap<String, Nest>,
}

impl Default for Nesting {
    fn default() -> Self {
        Self {
            inventory: Sheet::default().inventory,
            nests: BTreeMap::new(),
        }
    }
}

impl Nesting {
    pub(super) fn dimensional() -> Self {
        let mut nesting = Self::default();
        nesting.inventory.insert(
            "REM-STRIP".into(),
            Stock {
                lot: "LOT-AL-3".into(),
                alloy: "AL-5052".into(),
                area_mm2: 900,
                eligible: true,
                parent: Some("SHEET-AL-3".into()),
            },
        );
        nesting
    }

    pub(super) fn apply(
        &mut self,
        action: Action,
        args: &Map<String, Value>,
        events: &[Event],
        orders: &[String],
        setup: Setup,
    ) -> Value {
        if action == Action::TraceSheetMaterial {
            return self.trace(args, orders);
        }
        let id = args["nest_id"].as_str().unwrap();
        if action == Action::CreateSheetNest {
            return self.create(id, args, events, orders, setup);
        }
        let Some(nest) = self.nests.get_mut(id) else {
            return error("NEST_NOT_FOUND");
        };
        if action == Action::GetSheetNest {
            return json!({"ok":true,"nest":nest});
        }
        if action == Action::ReturnSheetRemnant {
            if nest.status != Status::Cut {
                return error("INVALID_STATE");
            }
            if nest.returned_remnant.is_some() {
                return error("REMNANT_ALREADY_RETURNED");
            }
            if nest.remnant_dimensions.is_none() {
                return error("NO_REUSABLE_REMNANT");
            }
            let remnant = args["remnant_id"].as_str().unwrap();
            if self.inventory.contains_key(remnant) {
                return error("STOCK_ID_EXISTS");
            }
            let source = &self.inventory[&nest.stock];
            self.inventory.insert(
                remnant.into(),
                Stock {
                    lot: source.lot.clone(),
                    alloy: source.alloy.clone(),
                    area_mm2: nest.remnant_mm2,
                    eligible: true,
                    parent: Some(nest.stock.clone()),
                },
            );
            nest.returned_remnant = Some(remnant.into());
            return json!({"ok":true,"nest":nest});
        }
        if nest.status != Status::Planned {
            return error("INVALID_STATE");
        }
        if action == Action::CancelSheetNest {
            nest.status = Status::Cancelled;
        } else {
            let machine = args["machine"].as_str().unwrap();
            let capable = nest.dimensional_fit.as_ref().map_or_else(
                || MACHINE_ENVELOPES.iter().any(|(known, _)| *known == machine),
                |fit| fit.eligible_machines.iter().any(|known| known == machine),
            );
            if !capable {
                return error("MACHINE_NOT_CAPABLE");
            }
            if event_matching(events, "sheet_machine_down", "machine", machine) {
                return error("MACHINE_UNAVAILABLE");
            }
            if event_matching(events, "sheet_supply_shortage", "stock", &nest.stock) {
                return error("STOCK_UNAVAILABLE");
            }
            self.inventory.get_mut(&nest.stock).unwrap().area_mm2 = 0;
            nest.machine = Some(machine.into());
            nest.status = Status::Cut;
        }
        json!({"ok":true,"nest":nest})
    }

    fn trace(&self, args: &Map<String, Value>, orders: &[String]) -> Value {
        let subject = args["subject"].as_str().unwrap();
        let kind = args["subject_type"].as_str().unwrap();
        let cuts = || {
            self.nests
                .values()
                .filter(|nest| nest.status == Status::Cut)
        };
        let selected: Vec<_> = match kind {
            "lot" if self.inventory.values().any(|stock| stock.lot == subject) => {
                cuts().filter(|nest| nest.lot == subject).collect()
            }
            "stock"
                if self.inventory.contains_key(subject)
                    || self
                        .inventory
                        .values()
                        .any(|stock| stock.parent.as_deref() == Some(subject)) =>
            {
                let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
                for (id, stock) in &self.inventory {
                    if let Some(parent) = &stock.parent {
                        children.entry(parent).or_default().push(id);
                    }
                }
                let mut pending = vec![subject];
                let mut lineage = BTreeSet::new();
                while let Some(stock) = pending.pop() {
                    if lineage.insert(stock)
                        && let Some(downstream) = children.get(stock)
                    {
                        pending.extend(downstream);
                    }
                }
                cuts()
                    .filter(|nest| lineage.contains(nest.stock.as_str()))
                    .collect()
            }
            "work_order" if orders.iter().any(|order| order == subject) => {
                let mut lineage = BTreeSet::new();
                if let Some(cut) = cuts().find(|nest| nest.order_area_mm2.contains_key(subject)) {
                    let mut source = Some(cut.stock.as_str());
                    while let Some(stock) = source.filter(|stock| lineage.insert(*stock)) {
                        source = self
                            .inventory
                            .get(stock)
                            .and_then(|entry| entry.parent.as_deref());
                    }
                }
                cuts()
                    .filter(|nest| lineage.contains(nest.stock.as_str()))
                    .collect()
            }
            _ => return error("TRACE_SUBJECT_NOT_FOUND"),
        };
        json!({"ok":true,"subject_type":kind,"subject":subject,"cuts":selected})
    }

    fn create(
        &mut self,
        id: &str,
        args: &Map<String, Value>,
        events: &[Event],
        orders: &[String],
        setup: Setup,
    ) -> Value {
        if self.nests.contains_key(id) {
            return error("DUPLICATE_NEST");
        }
        let work_orders = args["work_orders"].as_array().unwrap();
        let mut order_area_mm2 = BTreeMap::new();
        for order in work_orders {
            let order = order.as_str().unwrap();
            if !orders.iter().any(|known| known == order) {
                return error("WORK_ORDER_NOT_FOUND");
            }
            if order_area_mm2.insert(order.to_owned(), PART_AREA).is_some() {
                return error("DUPLICATE_WORK_ORDER");
            }
        }
        if self.nests.values().any(|nest| {
            nest.status != Status::Cancelled
                && order_area_mm2
                    .keys()
                    .any(|order| nest.order_area_mm2.contains_key(order))
        }) {
            return error("WORK_ORDER_ALLOCATED");
        }
        let stock_id = args["stock"].as_str().unwrap();
        let Some(stock) = self.inventory.get(stock_id) else {
            return error("STOCK_NOT_FOUND");
        };
        if event_matching(events, "sheet_supply_shortage", "stock", stock_id) {
            return error("STOCK_UNAVAILABLE");
        }
        if !stock.eligible || stock.alloy != "AL-5052" {
            return error("STOCK_INELIGIBLE");
        }
        if self
            .nests
            .values()
            .any(|nest| nest.status == Status::Planned && nest.stock == stock_id)
        {
            return error("STOCK_RESERVED");
        }
        let returned = self
            .nests
            .values()
            .find(|nest| nest.returned_remnant.as_deref() == Some(stock_id))
            .and_then(|nest| nest.remnant_dimensions);
        let dimensional_fit = (setup != Setup::SheetNesting)
            .then(|| DimensionalFit::for_stock(stock_id, &order_area_mm2, returned));
        if dimensional_fit.as_ref().is_some_and(|fit| {
            fit.order_footprints
                .values()
                .any(|footprint| !fit.stock_dimensions.contains(*footprint))
        }) {
            return error("STOCK_DIMENSIONS_INSUFFICIENT");
        }
        let product_mm2 = order_area_mm2.values().sum::<u64>();
        let Some(area_after_minimum_scrap) = stock.area_mm2.checked_sub(product_mm2 + SCRAP_AREA)
        else {
            return error("INSUFFICIENT_AREA");
        };
        let placements = if setup == Setup::SheetLayoutNesting {
            let fit = dimensional_fit
                .as_ref()
                .expect("layout requires dimensional stock");
            match layout::validated_plan(
                &layout::RowLayout,
                fit.stock_dimensions,
                &fit.order_footprints,
            ) {
                Ok(plan) => Some(plan),
                Err(code) => return error(code),
            }
        } else {
            None
        };
        // Preserve only a rectangular offcut outside the single row reserved for parts.
        let remnant_dimensions = dimensional_fit
            .as_ref()
            .filter(|_| setup.supports_remnants())
            .and_then(|fit| {
                let stock = fit.stock_dimensions;
                let ordered_length = placements.as_ref().map_or_else(
                    || {
                        fit.order_footprints
                            .values()
                            .map(|part| part.length_mm)
                            .sum()
                    },
                    layout::occupied_length,
                );
                let length = stock.length_mm.saturating_sub(
                    ordered_length.max((product_mm2 + SCRAP_AREA).div_ceil(stock.width_mm)),
                );
                (length > 0).then(|| Dimensions::new(length, stock.width_mm))
            });
        let remnant_mm2 = if setup.supports_remnants() {
            remnant_dimensions.map_or(0, |shape| shape.length_mm * shape.width_mm)
        } else {
            area_after_minimum_scrap
        };
        let nest = Nest {
            nest_id: id.into(),
            stock: stock_id.into(),
            lot: stock.lot.clone(),
            status: Status::Planned,
            order_area_mm2,
            input_mm2: stock.area_mm2,
            product_mm2,
            scrap_mm2: stock.area_mm2 - product_mm2 - remnant_mm2,
            remnant_mm2,
            machine: None,
            dimensional_fit,
            remnant_dimensions,
            returned_remnant: None,
            placements,
            parent_stock: (setup.supports_remnants())
                .then(|| stock.parent.clone())
                .flatten(),
        };
        let response = json!({"ok":true,"nest":nest});
        self.nests.insert(id.into(), nest);
        response
    }
}
