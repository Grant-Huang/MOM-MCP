use mom_des::{Candidate, Event, Intervention, Model};
use serde_json::{Value, json};

pub const IDS: [&str; 2] = ["smt", "assembly"];
pub const VERSION: u32 = 1;

pub struct Fixture;

impl Model for Fixture {
    fn initial(&self) -> Vec<Candidate> {
        ["fixture_ready", "fixture_observation"]
            .into_iter()
            .map(|kind| Candidate::Immediate {
                kind: kind.into(),
                payload: Value::Null,
            })
            .collect()
    }

    fn on_event(&self, _: &mut Event) -> Vec<Candidate> {
        vec![]
    }
    fn intervention(&self, _: &str) -> Option<Intervention> {
        None
    }
    fn diagnosis(&self) -> Value {
        json!({"root_cause":null})
    }
}
