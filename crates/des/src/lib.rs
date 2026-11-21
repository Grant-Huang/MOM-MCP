use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_STEP_EVENTS: usize = 100;
pub const MAX_STEP_MINUTES: u64 = 10_080;
const MAX_EVENTS: usize = 10_000;
const MAX_SAME_TICK: usize = 16;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "distribution", rename_all = "snake_case", deny_unknown_fields)]
pub enum Delay {
    Exponential { mean_minutes: f64 },
    Uniform { min_minutes: f64, max_minutes: f64 },
}

impl Delay {
    pub fn valid(&self) -> bool {
        let positive =
            |value: f64| value.is_finite() && value > 0.0 && value <= MAX_STEP_MINUTES as f64;
        match *self {
            Self::Exponential { mean_minutes } => positive(mean_minutes),
            Self::Uniform {
                min_minutes,
                max_minutes,
            } => positive(min_minutes) && positive(max_minutes) && min_minutes < max_minutes,
        }
    }

    fn sample(&self, uniform: f64) -> u64 {
        let minutes = match *self {
            Self::Exponential { mean_minutes } => -uniform.ln() * mean_minutes,
            Self::Uniform {
                min_minutes,
                max_minutes,
            } => min_minutes + uniform * (max_minutes - min_minutes),
        };
        // Positive delays round upward to whole minutes.
        minutes.ceil().clamp(1.0, u64::MAX as f64) as u64
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub event: String,
    pub delay: Delay,
}

pub enum Candidate {
    Immediate {
        kind: String,
        payload: Value,
    },
    Delayed {
        kind: String,
        stream: String,
        delay: Delay,
        payload: Value,
    },
}

pub trait Model {
    fn initial(&self) -> Vec<Candidate>;
    fn on_event(&self, event: &mut Event) -> Vec<Candidate>;
    fn intervention(&self, action: &str) -> Option<Intervention>;
    fn diagnosis(&self) -> Value;
}

pub struct Intervention {
    pub cancel: Vec<String>,
    pub schedule: Vec<Candidate>,
}

#[derive(Deserialize, Serialize)]
struct Pending {
    due: u64,
    sequence: u64,
    kind: String,
    payload: Value,
    rule: Option<usize>,
}

#[derive(Deserialize, Serialize)]
pub struct Event {
    pub cursor: usize,
    pub minute: u64,
    pub kind: String,
    pub payload: Value,
}

#[derive(Deserialize, Serialize)]
pub struct Timeline {
    pub clock: u64,
    seed: u64,
    sequence: u64,
    draws: BTreeMap<String, u64>,
    pending: Vec<Pending>,
    pub events: Vec<Event>,
}

impl Timeline {
    pub fn new(seed: u64, rules: &[Rule], model: &dyn Model) -> Self {
        let mut timeline = Self {
            clock: 0,
            seed,
            sequence: 0,
            draws: BTreeMap::new(),
            pending: vec![],
            events: vec![],
        };
        for (index, rule) in rules.iter().enumerate() {
            timeline.schedule_rule(index, rule);
        }
        timeline.schedule_all(model.initial());
        timeline
    }

    fn random(&mut self, stream: &str) -> f64 {
        let hash = stream.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
        let draw = self.draws.entry(stream.into()).or_default();
        *draw += 1;
        let mut value = self.seed ^ hash ^ draw.wrapping_mul(0x9e3779b97f4a7c15);
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^= value >> 31;
        (((value >> 12) as f64) + 0.5) / ((1_u64 << 52) as f64)
    }

    fn push(&mut self, delay: u64, kind: String, payload: Value, rule: Option<usize>) {
        self.sequence += 1;
        self.pending.push(Pending {
            due: self.clock.saturating_add(delay),
            sequence: self.sequence,
            kind,
            payload,
            rule,
        });
    }

    fn schedule_rule(&mut self, index: usize, rule: &Rule) {
        let delay = rule
            .delay
            .sample(self.random(&format!("rule:{index}:{}", rule.event)));
        self.push(delay, rule.event.clone(), Value::Null, Some(index));
    }

    fn schedule_all(&mut self, candidates: Vec<Candidate>) {
        for item in candidates {
            match item {
                Candidate::Immediate { kind, payload } => self.push(0, kind, payload, None),
                Candidate::Delayed {
                    kind,
                    stream,
                    delay,
                    payload,
                } => {
                    let minutes = delay.sample(self.random(&format!("model:{stream}")));
                    self.push(minutes, kind, payload, None);
                }
            }
        }
    }

    pub fn apply(&mut self, intervention: Intervention) {
        self.pending
            .retain(|event| event.rule.is_some() || !intervention.cancel.contains(&event.kind));
        self.schedule_all(intervention.schedule);
    }

    pub fn advance(
        &mut self,
        event_budget: usize,
        minutes: Option<u64>,
        rules: &[Rule],
        model: &dyn Model,
    ) -> &'static str {
        let until = minutes.map(|minutes| self.clock.saturating_add(minutes));
        for _ in 0..event_budget {
            if self.events.len() >= MAX_EVENTS {
                return "history_limit";
            }
            let next = self
                .pending
                .iter()
                .enumerate()
                .min_by_key(|(_, event)| (event.due, event.sequence));
            let Some((index, pending)) = next else {
                return "queue_empty";
            };
            if let Some(until) = until.filter(|until| pending.due > *until) {
                self.clock = until;
                return "time_budget";
            }
            if self
                .events
                .iter()
                .rev()
                .take_while(|event| event.minute == pending.due)
                .count()
                >= MAX_SAME_TICK
            {
                return "same_tick_limit";
            }
            let pending = self.pending.swap_remove(index);
            self.clock = pending.due;
            let mut event = Event {
                cursor: self.events.len() + 1,
                minute: self.clock,
                kind: pending.kind,
                payload: pending.payload,
            };
            if let Some(index) = pending.rule {
                self.schedule_rule(index, &rules[index]);
            } else {
                self.schedule_all(model.on_event(&mut event));
            }
            self.events.push(event);
        }
        "event_budget"
    }
}
