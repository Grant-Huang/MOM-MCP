use mom_des::{Candidate, Delay, Event, Intervention, Model};
use serde::Deserialize;
use serde_json::{Value, json};

pub const SHEET: &str = "sheet_metal";
pub const HARNESS: &str = "wire_harness";
pub const VERSION: u32 = 1;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parameters {
    mean_minutes: f64,
}

pub struct Fabrication {
    id: String,
    parameters: Parameters,
}

impl Fabrication {
    pub fn load(id: &str, parameters: Value) -> Result<Self, String> {
        let parameters: Parameters =
            serde_json::from_value(parameters).map_err(|e| e.to_string())?;
        if !(Delay::Exponential {
            mean_minutes: parameters.mean_minutes,
        })
        .valid()
        {
            return Err("invalid mean_minutes".into());
        }
        Ok(Self {
            id: id.into(),
            parameters,
        })
    }

    fn event(&self, kind: &str, payload: Value) -> Candidate {
        Candidate::Delayed {
            kind: kind.into(),
            stream: kind.into(),
            delay: Delay::Exponential {
                mean_minutes: self.parameters.mean_minutes,
            },
            payload,
        }
    }
}

impl Model for Fabrication {
    fn initial(&self) -> Vec<Candidate> {
        if self.id == SHEET {
            vec![
                self.event("sheet_supply_shortage", json!({"stock":"SHEET-FULL"})),
                self.event("sheet_machine_down", json!({"machine":"LASER-A"})),
            ]
        } else {
            vec![self.event(
                "harness_sample_failed",
                json!({"terminal_batch":"TERM-ALT","serial":"WIRE-01"}),
            )]
        }
    }
    fn on_event(&self, _: &mut Event) -> Vec<Candidate> {
        vec![]
    }
    fn intervention(&self, _: &str) -> Option<Intervention> {
        None
    }
    fn diagnosis(&self) -> Value {
        json!({"model":self.id})
    }
}
