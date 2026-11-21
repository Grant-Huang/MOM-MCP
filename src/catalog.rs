use rmcp::model::{Tool, ToolAnnotations};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::scenario::Rejection;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    GetWorkOrder,
    GetMaterial,
    HoldMaterial,
}

pub struct Gateway {
    pub name: &'static str,
    description: &'static str,
    read_only: bool,
    pub input: Value,
    pub control: bool,
}

#[derive(Serialize)]
pub struct Spec {
    name: &'static str,
    description: &'static str,
    domain: &'static str,
    effect: &'static str,
    pub input: Value,
    output: Value,
    preconditions: &'static [&'static str],
    side_effect: &'static str,
    errors: &'static [&'static str],
    visibility: &'static str,
}

fn object(required: &[&str], properties: Value) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

fn text() -> Value {
    json!({"type":"string","minLength":1,"maxLength":256})
}

impl Action {
    pub fn named(name: &str) -> Option<Self> {
        [Self::GetWorkOrder, Self::GetMaterial, Self::HoldMaterial]
            .into_iter()
            .find(|action| action.name() == name)
    }

    fn name(self) -> &'static str {
        match self {
            Self::GetWorkOrder => "get_work_order",
            Self::GetMaterial => "get_material",
            Self::HoldMaterial => "hold_material",
        }
    }

    fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::GetWorkOrder => &["工单", "work order", "order"],
            Self::GetMaterial => &["物料", "material", "batch", "批次", "inventory"],
            Self::HoldMaterial => &[
                "质量冻结",
                "隔离",
                "暂扣",
                "hold",
                "quality hold",
                "quarantine",
            ],
        }
    }

    pub fn spec(self) -> Spec {
        let (description, domain, effect, input, fields, side_effect, errors) = match self {
            Self::GetWorkOrder => (
                "查询工单",
                "production",
                "read",
                object(&["work_order"], json!({"work_order":text()})),
                json!({"work_order":text(),"status":{"type":"string"}}),
                "none",
                &["INVALID_ARGUMENT", "WORK_ORDER_NOT_FOUND"][..],
            ),
            Self::GetMaterial => (
                "查询物料批次及冻结状态",
                "inventory",
                "read",
                object(&["batch"], json!({"batch":text()})),
                json!({"batch":text(),"work_order":text(),"held":{"type":"boolean"},"reason":{"type":["null","string"]},"cursor":{"type":"integer"}}),
                "none",
                &["INVALID_ARGUMENT", "MATERIAL_NOT_FOUND"][..],
            ),
            Self::HoldMaterial => (
                "质量冻结物料批次",
                "quality",
                "write",
                object(
                    &["batch", "reason", "idempotency_key"],
                    json!({"batch":text(),"reason":text(),"idempotency_key":text()}),
                ),
                json!({"batch":text(),"held":{"type":"boolean"},"reason":text(),"cursor":{"type":"integer"}}),
                "freeze_batch",
                &[
                    "INVALID_ARGUMENT",
                    "MATERIAL_NOT_FOUND",
                    "ALREADY_HELD",
                    "IDEMPOTENCY_CONFLICT",
                ][..],
            ),
        };
        let mut fields = fields.as_object().unwrap().clone();
        fields.insert("ok".into(), json!({"const":true}));
        fields.insert("run_id".into(), text());
        let required: Vec<_> = fields.keys().map(String::as_str).collect();
        Spec {
            name: self.name(),
            description,
            domain,
            effect,
            input,
            output: json!({"oneOf":[object(&required, Value::Object(fields.clone())),
                object(&["ok","code","field"],json!({"ok":{"const":false},"code":text(),"field":text()}))]}),
            preconditions: &["run_open", "tool_enabled_in_scenario"],
            side_effect,
            errors,
            visibility: "run",
        }
    }
}

pub fn search(capabilities: &[Action], args: &Map<String, Value>) -> Vec<Value> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let mut results: Vec<_> = capabilities.iter().filter_map(|action| {
        let spec = action.spec();
        if args.get("domain").is_some_and(|v| v != spec.domain) || args.get("effect").is_some_and(|v| v != spec.effect) {
            return None;
        }
        let score = if query.is_empty() { 0 }
        else if spec.name == query || action.keywords().contains(&query.as_str()) { 2 }
        else if spec.name.contains(&query) || spec.description.contains(&query)
             || action.keywords().iter().any(|keyword| keyword.contains(&query))
             || spec.input["properties"].as_object().unwrap().keys().any(|field| field.contains(&query)) { 1 }
        else { return None; };
        Some((score,spec.name,json!({"name":spec.name,"description":spec.description,"domain":spec.domain,"effect":spec.effect})))
    }).collect();
    results.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    results.into_iter().map(|(_, _, item)| item).collect()
}

pub const NAMES: [&str; 11] = [
    "list_scenarios",
    "open_scenario",
    "get_run",
    "close_run",
    "search_tools",
    "describe_tool",
    "call_tool",
    "read_events",
    "advance_run",
    "intervene_run",
    "inspect_run",
];

pub fn gateway(name: &str) -> Option<Gateway> {
    let (name, description, read_only, input) = match name {
        "list_scenarios" => (
            "list_scenarios",
            "List published mock scenarios",
            true,
            object(&[], json!({})),
        ),
        "open_scenario" => (
            "open_scenario",
            "Open an isolated scenario run",
            false,
            object(
                &["scenario_id", "version"],
                json!({
                    "scenario_id":text(),"version":{"type":"integer","minimum":1,"maximum":u32::MAX},"seed":{"type":"integer","minimum":0}
                }),
            ),
        ),
        "get_run" => (
            "get_run",
            "Read run metadata",
            true,
            object(&["run_id"], json!({"run_id":text()})),
        ),
        "close_run" => (
            "close_run",
            "Close a run",
            false,
            object(&["run_id"], json!({"run_id":text()})),
        ),
        "search_tools" => (
            "search_tools",
            "Search the current run's business tools",
            true,
            object(
                &["run_id"],
                json!({
                    "run_id":text(),"query":{"type":"string","maxLength":256},"domain":{"type":"string","enum":["production","inventory","quality"]},
                    "effect":{"type":"string","enum":["read","write"]}
                }),
            ),
        ),
        "describe_tool" => (
            "describe_tool",
            "Inspect a business tool contract",
            true,
            object(&["run_id", "name"], json!({"run_id":text(),"name":text()})),
        ),
        "call_tool" => (
            "call_tool",
            "Call a business tool within this run",
            false,
            object(
                &["run_id", "name", "arguments"],
                json!({
                    "run_id":text(),"name":text(),"arguments":{"type":"object"}
                }),
            ),
        ),
        "read_events" => (
            "read_events",
            "Read already delivered events",
            true,
            object(
                &["run_id"],
                json!({
                    "run_id":text(),"after":{"type":"integer","maximum":10000},"limit":{"type":"integer","minimum":1,"maximum":100}
                }),
            ),
        ),
        "advance_run" => (
            "advance_run",
            "Advance a trusted simulation within a work budget",
            false,
            object(
                &["run_id"],
                json!({
                    "run_id":text(),"max_events":{"type":"integer","minimum":1,"maximum":mom_des::MAX_STEP_EVENTS},
                    "max_minutes":{"type":"integer","minimum":1,"maximum":mom_des::MAX_STEP_MINUTES}
                }),
            ),
        ),
        "intervene_run" => (
            "intervene_run",
            "Schedule an intervention at the current virtual time",
            false,
            object(
                &["run_id", "action"],
                json!({"run_id":text(),"action":text()}),
            ),
        ),
        "inspect_run" => (
            "inspect_run",
            "Inspect hidden evaluation state and replay controls",
            true,
            object(&["run_id"], json!({"run_id":text()})),
        ),
        _ => return None,
    };
    Some(Gateway {
        name,
        description,
        read_only,
        input,
        control: matches!(name, "advance_run" | "intervene_run" | "inspect_run"),
    })
}

pub fn tools(control: bool) -> Vec<Tool> {
    NAMES
        .iter()
        .filter_map(|name| {
            let spec = gateway(name).filter(|spec| control || !spec.control)?;
            let mut tool = Tool::new(
                spec.name,
                spec.description,
                spec.input.as_object().unwrap().clone(),
            );
            tool.annotations = Some(
                ToolAnnotations::new()
                    .read_only(spec.read_only)
                    .destructive(false),
            );
            Some(tool)
        })
        .collect()
}

// The registry uses a limited schema vocabulary, checked here for both gateway and business inputs.
pub fn validate(schema: &Value, args: &Map<String, Value>) -> Result<(), Rejection> {
    let properties = schema["properties"].as_object().unwrap();
    for required in schema["required"].as_array().unwrap() {
        let field = required.as_str().unwrap();
        if !args.contains_key(field) {
            return Err(Rejection::invalid(field));
        }
    }
    for (name, value) in args {
        let Some(rule) = properties.get(name) else {
            return Err(Rejection::invalid(name));
        };
        let valid = match rule["type"].as_str().unwrap() {
            "object" => value.is_object(),
            "integer" => value.as_u64().is_some_and(|n| {
                n >= rule["minimum"].as_u64().unwrap_or(0)
                    && n <= rule["maximum"].as_u64().unwrap_or(u64::MAX)
            }),
            "string" => value.as_str().is_some_and(|s| {
                let len = s.chars().count() as u64;
                len >= rule["minLength"].as_u64().unwrap_or(0)
                    && len <= rule["maxLength"].as_u64().unwrap_or(u64::MAX)
                    && rule["enum"]
                        .as_array()
                        .is_none_or(|values| values.contains(value))
            }),
            _ => unreachable!("unsupported registry schema"),
        };
        if !valid {
            return Err(Rejection::invalid(name));
        }
    }
    Ok(())
}
