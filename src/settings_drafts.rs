use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub(crate) const CREATE: &str = "create_mes_settings_draft";
pub(crate) const VALIDATE: &str = "validate_mes_settings_draft";
pub(crate) const PREVIEW: &str = "preview_mes_settings_draft";

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Center {
    work_center: String,
    active: bool,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Shift {
    shift_id: String,
    work_center: String,
    start_local: String,
    end_local: String,
}

#[derive(PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    draft_id: String,
    base_version: u64,
    idempotency_key: String,
    work_centers: Vec<Center>,
    shifts: Vec<Shift>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lookup {
    draft_id: String,
}

#[derive(Default)]
pub(crate) struct Drafts {
    drafts: BTreeMap<String, Draft>,
    keys: BTreeMap<String, String>,
}

pub(crate) fn schema(name: &str) -> Value {
    let text = json!({"type":"string","minLength":1,"maxLength":128});
    if name != CREATE {
        return json!({"type":"object","additionalProperties":false,"required":["draft_id"],
            "properties":{"draft_id":text}});
    }
    json!({"type":"object","additionalProperties":false,
    "required":["draft_id","base_version","idempotency_key","work_centers","shifts"],
    "properties":{"draft_id":text,"base_version":{"type":"integer","minimum":1},"idempotency_key":text,
        "work_centers":{"type":"array","minItems":1,"maxItems":50,"items":{
            "type":"object","additionalProperties":false,"required":["work_center","active"],
            "properties":{"work_center":text,"active":{"type":"boolean"}}}},
        "shifts":{"type":"array","maxItems":50,"items":{
            "type":"object","additionalProperties":false,"required":["shift_id","work_center","start_local","end_local"],
            "properties":{"shift_id":text,"work_center":text,"start_local":text,"end_local":text}}}
    }})
}

pub(crate) fn error(code: &str) -> Value {
    json!({"ok":false,"code":code})
}

impl Drafts {
    pub(crate) fn invoke(&mut self, name: &str, args: Map<String, Value>, active: &Value) -> Value {
        if name == CREATE {
            let Ok(draft) = serde_json::from_value::<Draft>(Value::Object(args)) else {
                return error("INVALID_ARGUMENT");
            };
            if !identifier(&draft.draft_id)
                || draft.idempotency_key.trim().is_empty()
                || draft.idempotency_key.len() > 128
                || draft.work_centers.is_empty()
                || draft.work_centers.len() > 50
                || draft.shifts.len() > 50
                || draft
                    .work_centers
                    .iter()
                    .any(|center| !identifier(&center.work_center))
                || draft.shifts.iter().any(|shift| {
                    !identifier(&shift.shift_id)
                        || !identifier(&shift.work_center)
                        || shift.start_local.len() > 16
                        || shift.end_local.len() > 16
                })
            {
                return error("INVALID_ARGUMENT");
            }
            if let Some(id) = self.keys.get(&draft.idempotency_key) {
                let saved = &self.drafts[id];
                return if saved == &draft {
                    created(saved)
                } else {
                    error("IDEMPOTENCY_CONFLICT")
                };
            }
            if draft.base_version != active["active_version"] {
                return error("VERSION_CONFLICT");
            }
            if self.drafts.contains_key(&draft.draft_id) {
                return error("DRAFT_ALREADY_EXISTS");
            }
            let reply = created(&draft);
            self.keys
                .insert(draft.idempotency_key.clone(), draft.draft_id.clone());
            self.drafts.insert(draft.draft_id.clone(), draft);
            return reply;
        }

        let Ok(lookup) = serde_json::from_value::<Lookup>(Value::Object(args)) else {
            return error("INVALID_ARGUMENT");
        };
        if !identifier(&lookup.draft_id) {
            return error("INVALID_ARGUMENT");
        }
        let Some(draft) = self.drafts.get(&lookup.draft_id) else {
            return error("DRAFT_NOT_FOUND");
        };
        let issues = draft.issues();
        if name == VALIDATE {
            return json!({"ok":true,"draft_id":draft.draft_id,"valid":issues.is_empty(),"issues":issues});
        }
        if !issues.is_empty() {
            return json!({"ok":false,"code":"DRAFT_INVALID","issues":issues});
        }
        let proposed = json!({"work_centers":draft.work_centers,"shifts":draft.shifts,"timezone":active["timezone"]});
        let digest = Sha256::digest(
            json!({
                "site_id":active["site_id"],"base_version":draft.base_version,"settings":proposed,
            })
            .to_string(),
        );
        let fingerprint: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        let changed: Vec<_> = ["work_centers", "shifts"]
            .into_iter()
            .filter(|field| active[field] != proposed[field])
            .collect();
        json!({"ok":true,"draft_id":draft.draft_id,"base_version":draft.base_version,
            "candidate_fingerprint":fingerprint,"changed":changed,"proposed":proposed,
            "activation_scope":"future_runs","existing_runs_affected":false})
    }
}

impl Draft {
    fn issues(&self) -> Vec<Value> {
        let mut issues = Vec::new();
        let mut centers = BTreeMap::new();
        for center in &self.work_centers {
            if centers
                .insert(center.work_center.as_str(), center.active)
                .is_some()
            {
                issues
                    .push(json!({"code":"DUPLICATE_WORK_CENTER","work_center":center.work_center}));
            }
        }
        let mut shift_ids = BTreeSet::new();
        let mut intervals = BTreeMap::<&str, Vec<(u16, u16, &str)>>::new();
        for shift in &self.shifts {
            if !shift_ids.insert(&shift.shift_id) {
                issues.push(json!({"code":"DUPLICATE_SHIFT","shift_id":shift.shift_id}));
            }
            if centers.get(shift.work_center.as_str()) != Some(&true) {
                issues.push(json!({"code":"WORK_CENTER_UNAVAILABLE","shift_id":shift.shift_id}));
            }
            let Some((start, end)) = minute(&shift.start_local)
                .zip(minute(&shift.end_local))
                .filter(|(a, b)| a != b)
            else {
                issues.push(json!({"code":"INVALID_SHIFT_TIME","shift_id":shift.shift_id}));
                continue;
            };
            let ranges = if start < end {
                [(start, end), (0, 0)]
            } else {
                [(start, 1440), (0, end)]
            };
            let recorded = intervals.entry(&shift.work_center).or_default();
            for (start, end) in ranges.into_iter().filter(|(a, b)| a < b) {
                for &(previous_start, previous_end, previous_id) in recorded.iter() {
                    if start < previous_end && previous_start < end {
                        issues.push(json!({"code":"SHIFT_OVERLAP","shift_id":shift.shift_id,"conflicts_with":previous_id}));
                    }
                }
                recorded.push((start, end, &shift.shift_id));
            }
        }
        issues
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn minute(time: &str) -> Option<u16> {
    let (h, m) = time.split_once(':')?;
    if h.len() != 2 || m.len() != 2 || !h.bytes().chain(m.bytes()).all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let (h, m) = (h.parse::<u16>().ok()?, m.parse::<u16>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

fn created(draft: &Draft) -> Value {
    json!({"ok":true,"draft_id":draft.draft_id,"base_version":draft.base_version})
}
