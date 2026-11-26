use std::collections::BTreeMap;

use super::{
    Action, Event, Map, PART_AREA, SCRAP_AREA, Sheet, Spec, Stock, Value, error, event_matching,
    json, object, text,
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
    if action == Action::CutSheetNest {
        required.push("machine");
        fields["machine"] = text();
    }
    let area = json!({"type":"integer","minimum":0});
    let nest = object(
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
            "input_mm2":area,"product_mm2":area,"scrap_mm2":area,"remnant_mm2":area,"machine":text()}),
    );
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
            "NEST_NOT_FOUND",
            "INVALID_STATE",
            "MACHINE_NOT_CAPABLE",
            "MACHINE_UNAVAILABLE",
            "IDEMPOTENCY_CONFLICT",
        ],
        visibility: "run",
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Status {
    Planned,
    Cut,
    Cancelled,
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
    pub(super) fn apply(
        &mut self,
        action: Action,
        args: &Map<String, Value>,
        events: &[Event],
        orders: &[String],
    ) -> Value {
        let id = args["nest_id"].as_str().unwrap();
        if action == Action::CreateSheetNest {
            return self.create(id, args, events, orders);
        }
        let Some(nest) = self.nests.get_mut(id) else {
            return error("NEST_NOT_FOUND");
        };
        if action == Action::GetSheetNest {
            return json!({"ok":true,"nest":nest});
        }
        if nest.status != Status::Planned {
            return error("INVALID_STATE");
        }
        if action == Action::CancelSheetNest {
            nest.status = Status::Cancelled;
        } else {
            let machine = args["machine"].as_str().unwrap();
            if !matches!(machine, "LASER-A" | "LASER-B") {
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

    fn create(
        &mut self,
        id: &str,
        args: &Map<String, Value>,
        events: &[Event],
        orders: &[String],
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
        let product_mm2 = order_area_mm2.values().sum::<u64>();
        let Some(remnant_mm2) = stock.area_mm2.checked_sub(product_mm2 + SCRAP_AREA) else {
            return error("INSUFFICIENT_AREA");
        };
        let nest = Nest {
            nest_id: id.into(),
            stock: stock_id.into(),
            lot: stock.lot.clone(),
            status: Status::Planned,
            order_area_mm2,
            input_mm2: stock.area_mm2,
            product_mm2,
            scrap_mm2: SCRAP_AREA,
            remnant_mm2,
            machine: None,
        };
        let response = json!({"ok":true,"nest":nest});
        self.nests.insert(id.into(), nest);
        response
    }
}
