use mom_des::{Candidate, Delay, Event, Intervention, Model};
use serde::Deserialize;
use serde_json::{Value, json};

pub const ID: &str = "cnc_wear";
pub const VERSION: u32 = 1;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cnc {
    alarm_mean_minutes: f64,
    repair_mean_minutes: f64,
    wear_per_minute: f64,
}

impl Default for Cnc {
    fn default() -> Self {
        Self {
            alarm_mean_minutes: 24.0,
            repair_mean_minutes: 5.0,
            wear_per_minute: 0.03,
        }
    }
}

impl Cnc {
    pub fn load(parameters: Value) -> Result<Self, String> {
        let model: Self = serde_json::from_value(parameters).map_err(|err| err.to_string())?;
        for (name, value) in [
            ("alarm_mean_minutes", model.alarm_mean_minutes),
            ("repair_mean_minutes", model.repair_mean_minutes),
        ] {
            if !(Delay::Exponential {
                mean_minutes: value,
            })
            .valid()
            {
                return Err(format!("{name} must be positive and bounded"));
            }
        }
        if !model.wear_per_minute.is_finite()
            || !(0.0 < model.wear_per_minute && model.wear_per_minute <= 1.0)
        {
            return Err("wear_per_minute must be in (0, 1]".into());
        }
        Ok(model)
    }

    fn alarm(&self, cycle_start: u64) -> Candidate {
        Candidate::Delayed {
            kind: "cnc_vibration".into(),
            stream: "vibration".into(),
            delay: Delay::Exponential {
                mean_minutes: self.alarm_mean_minutes,
            },
            payload: json!({"cycle_start":cycle_start}),
        }
    }
}

impl Model for Cnc {
    fn initial(&self) -> Vec<Candidate> {
        vec![self.alarm(0)]
    }

    fn on_event(&self, event: &mut Event) -> Vec<Candidate> {
        match event.kind.as_str() {
            "cnc_repair_completed" => vec![self.alarm(event.minute)],
            "cnc_vibration" => {
                // Mock wear and process capability are derived from time since maintenance.
                let age = event.minute
                    - event.payload["cycle_start"]
                        .as_u64()
                        .expect("CNC cycle start");
                let wear = age as f64 * self.wear_per_minute;
                let vibration = 1.8 + 2.0 * wear;
                let excess = (wear - 0.4).max(0.0);
                let cpk = ((0.025 - 0.012 * excess) / (0.009 * (1.0 + 0.35 * excess))).max(0.0);
                event.payload =
                    json!({"asset":"CNC-1","severity":"warning","vibration_mm_s":round(vibration)});
                vec![Candidate::Immediate {
                    kind: "cnc_quality_check".into(),
                    payload: json!({"asset":"CNC-1","inspection":"required","cpk":round(cpk)}),
                }]
            }
            _ => vec![],
        }
    }

    fn intervention(&self, action: &str) -> Option<Intervention> {
        (action == "maintenance").then(|| Intervention {
            cancel: vec!["cnc_vibration".into(), "cnc_repair_completed".into()],
            schedule: vec![
                Candidate::Immediate {
                    kind: "cnc_repair_started".into(),
                    payload: json!({"asset":"CNC-1"}),
                },
                Candidate::Delayed {
                    kind: "cnc_repair_completed".into(),
                    stream: "maintenance".into(),
                    delay: Delay::Exponential {
                        mean_minutes: self.repair_mean_minutes,
                    },
                    payload: json!({"asset":"CNC-1"}),
                },
            ],
        })
    }

    fn diagnosis(&self) -> Value {
        json!({"root_cause":"spindle_wear","asset":"CNC-1"})
    }
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}
