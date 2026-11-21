use mom_des::{Candidate, Delay, Event, Intervention, Model};
use serde::Deserialize;
use serde_json::{Value, json};

pub const ID: &str = "smt_line";
pub const VERSION: u32 = 1;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Smt {
    recall_reel: String,
    recall_mean_minutes: f64,
    aoi_panel: String,
    aoi_mean_minutes: f64,
}

impl Smt {
    pub fn load(parameters: Value) -> Result<Self, String> {
        let model: Self = serde_json::from_value(parameters).map_err(|error| error.to_string())?;
        for (field, mean_minutes) in [
            ("recall_mean_minutes", model.recall_mean_minutes),
            ("aoi_mean_minutes", model.aoi_mean_minutes),
        ] {
            if !(Delay::Exponential { mean_minutes }).valid() {
                return Err(format!("invalid {field}"));
            }
        }
        if model.recall_reel.is_empty() || model.aoi_panel.is_empty() {
            return Err("recall_reel and aoi_panel must be nonempty".into());
        }
        Ok(model)
    }
}

impl Model for Smt {
    fn initial(&self) -> Vec<Candidate> {
        [
            (
                "supplier_recall",
                self.recall_mean_minutes,
                json!({"reel":self.recall_reel}),
            ),
            (
                "aoi_drift",
                self.aoi_mean_minutes,
                json!({"panel":self.aoi_panel,"station":"AOI-1"}),
            ),
        ]
        .map(|(kind, mean_minutes, payload)| Candidate::Delayed {
            kind: kind.into(),
            stream: kind.into(),
            delay: Delay::Exponential { mean_minutes },
            payload,
        })
        .into()
    }
    fn on_event(&self, _: &mut Event) -> Vec<Candidate> {
        vec![]
    }
    fn intervention(&self, _: &str) -> Option<Intervention> {
        None
    }
    fn diagnosis(&self) -> Value {
        json!({"recalled_reel":self.recall_reel,"aoi_panel":self.aoi_panel})
    }
}
