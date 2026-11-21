use mom_des::{Model, Rule, Timeline};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub fn load_model(
    id: &str,
    version: u32,
    parameters: &Map<String, Value>,
) -> Result<Box<dyn Model>, String> {
    match id {
        mom_model_cnc::ID if version == mom_model_cnc::VERSION => {
            mom_model_cnc::Cnc::load(Value::Object(parameters.clone()))
                .map(|model| Box::new(model) as Box<dyn Model>)
        }
        mom_model_smt::ID if version == mom_model_smt::VERSION => {
            mom_model_smt::Smt::load(Value::Object(parameters.clone()))
                .map(|model| Box::new(model) as Box<dyn Model>)
        }
        mom_model_assembly::ID if version == mom_model_assembly::VERSION => {
            mom_model_assembly::Assembly::load(Value::Object(parameters.clone()))
                .map(|model| Box::new(model) as Box<dyn Model>)
        }
        id if matches!(
            id,
            mom_model_fabrication::SHEET | mom_model_fabrication::HARNESS
        ) && version == mom_model_fabrication::VERSION =>
        {
            mom_model_fabrication::Fabrication::load(id, Value::Object(parameters.clone()))
                .map(|model| Box::new(model) as Box<dyn Model>)
        }
        id if mom_model_fixture::IDS.contains(&id) && version == mom_model_fixture::VERSION => {
            if !parameters.is_empty() {
                return Err("fixture does not accept model parameters".into());
            }
            Ok(Box::new(mom_model_fixture::Fixture))
        }
        _ => Err(format!("unknown model or model_version: {id}@{version}")),
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Advance {
        event_budget: usize,
        minutes: Option<u64>,
    },
    Intervene {
        action: String,
    },
}

#[derive(Deserialize, Serialize)]
pub struct Simulation {
    pub timeline: Timeline,
    pub commands: Vec<Command>,
}

impl Simulation {
    pub fn new(seed: u64, rules: &[Rule], model: &dyn Model) -> Self {
        Self {
            timeline: Timeline::new(seed, rules, model),
            commands: vec![],
        }
    }

    pub fn execute(
        &mut self,
        command: Command,
        rules: &[Rule],
        model: &dyn Model,
    ) -> Result<&'static str, &'static str> {
        if self.commands.len() >= 10_000 {
            return Err("COMMAND_LIMIT");
        }
        let status = match &command {
            Command::Advance {
                event_budget,
                minutes,
            } => self.timeline.advance(*event_budget, *minutes, rules, model),
            Command::Intervene { action } => {
                self.timeline.apply(
                    model
                        .intervention(action)
                        .ok_or("INTERVENTION_UNSUPPORTED")?,
                );
                "intervention_scheduled"
            }
        };
        self.commands.push(command);
        Ok(status)
    }

    pub fn verify_replay(
        &self,
        seed: u64,
        rules: &[Rule],
        model: &dyn Model,
    ) -> anyhow::Result<bool> {
        let mut replay = Self::new(seed, rules, model);
        for command in self.commands.iter().cloned() {
            replay
                .execute(command, rules, model)
                .map_err(|reason| anyhow::anyhow!("{reason}"))?;
        }
        Ok(serde_json::to_value(replay.timeline)? == serde_json::to_value(&self.timeline)?)
    }

    pub fn page(&self, after: usize, limit: usize) -> Value {
        let events: Vec<_> = self
            .timeline
            .events
            .iter()
            .skip(after)
            .take(limit)
            .collect();
        let cursor = events.last().map_or(after, |event| event.cursor);
        json!({"ok":true,"clock":self.timeline.clock,"events":events,"event_cursor":cursor,
            "has_more":cursor < self.timeline.events.len()})
    }
}
