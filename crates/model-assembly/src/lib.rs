use mom_des::{Candidate, Delay, Event, Intervention, Model};
use serde::Deserialize;
use serde_json::{Value, json};

pub const ID: &str = "configure_to_order";
pub const VERSION: u32 = 1;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assembly {
    test_mean_minutes: f64,
    delivery_mean_minutes: Option<f64>,
    fault_order: Option<String>,
}

impl Assembly {
    pub fn load(parameters: Value) -> Result<Self, String> {
        let model: Self = serde_json::from_value(parameters).map_err(|e| e.to_string())?;
        for (name, value) in [
            ("test_mean_minutes", Some(model.test_mean_minutes)),
            ("delivery_mean_minutes", model.delivery_mean_minutes),
        ] {
            if value.is_some_and(|mean_minutes| !(Delay::Exponential { mean_minutes }).valid()) {
                return Err(format!("invalid {name}"));
            }
        }
        Ok(model)
    }
}

impl Model for Assembly {
    fn initial(&self) -> Vec<Candidate> {
        let mut events = Vec::new();
        if let Some(mean_minutes) = self.delivery_mean_minutes {
            events.push(Candidate::Delayed {
                kind: "assembly_receipt".into(),
                stream: "delivery".into(),
                delay: Delay::Exponential { mean_minutes },
                payload: json!({"part":"DRIVE-STD","quantity":1}),
            });
        }
        if let Some(order) = &self.fault_order {
            events.push(Candidate::Delayed {
                kind: "assembly_test_fault".into(),
                stream: "quality".into(),
                delay: Delay::Uniform {
                    min_minutes: 0.1,
                    max_minutes: 0.2,
                },
                payload: json!({"work_order":order}),
            });
        }
        events
    }
    fn on_event(&self, _: &mut Event) -> Vec<Candidate> {
        vec![]
    }
    fn intervention(&self, action: &str) -> Option<Intervention> {
        let ticket = action.strip_prefix("assembly_test:")?.parse::<u64>().ok()?;
        Some(Intervention {
            cancel: vec![],
            schedule: vec![Candidate::Delayed {
                kind: "assembly_test_complete".into(),
                stream: "test_duration".into(),
                delay: Delay::Exponential {
                    mean_minutes: self.test_mean_minutes,
                },
                payload: json!({"ticket":ticket}),
            }],
        })
    }
    fn diagnosis(&self) -> Value {
        json!({"fault_order":self.fault_order})
    }
}
