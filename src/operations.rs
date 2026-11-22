use serde::Serialize;

#[derive(Serialize)]
pub struct Operation {
    pub work_order: String,
    pub operation: &'static str,
    pub sequence: usize,
    pub status: &'static str,
    pub work_center: Option<String>,
    pub blocking: Option<&'static str>,
    pub priority: u64,
    #[serde(skip)]
    pub due: u64,
}
