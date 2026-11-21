use std::{path::PathBuf, time::Duration};

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Map, Value, json};

#[derive(Clone)]
pub struct Probe {
    db: PathBuf,
    response_delay_ms: u64,
}

impl Probe {
    pub fn new(db: PathBuf, response_delay_ms: u64) -> anyhow::Result<Self> {
        let probe = Self {
            db,
            response_delay_ms,
        };
        probe.connect()?.execute(
            "CREATE TABLE IF NOT EXISTS writes (key TEXT PRIMARY KEY, value INTEGER NOT NULL)",
            [],
        )?;
        Ok(probe)
    }

    fn connect(&self) -> rusqlite::Result<Connection> {
        let db = Connection::open(&self.db)?;
        db.busy_timeout(Duration::from_secs(2))?;
        Ok(db)
    }

    fn run(&self, name: &str, args: Map<String, Value>) -> rusqlite::Result<(Value, bool)> {
        let mut db = self.connect()?;
        if name == "get_probe" {
            let (revision, value) = current(&db)?;
            let code = if args.is_empty() {
                "OK"
            } else {
                "INVALID_ARGUMENT"
            };
            return Ok((result(code, revision, value), false));
        }

        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (revision, existing) = current(&tx)?;
        let value = args.get("value").and_then(Value::as_i64);
        let key = args
            .get("idempotency_key")
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty() && key.chars().count() <= 128);
        let (Some(value), Some(key)) = (value, key) else {
            return Ok((result("INVALID_ARGUMENT", revision, existing), false));
        };
        if args.len() != 2 {
            return Ok((result("INVALID_ARGUMENT", revision, existing), false));
        }
        if !(0..=100).contains(&value) {
            return Ok((result("VALUE_OUT_OF_RANGE", revision, existing), false));
        }
        let previous = tx
            .query_row(
                "SELECT rowid, value FROM writes WHERE key = ?",
                [key],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        if let Some((saved_revision, saved_value)) = previous {
            return Ok((
                if saved_value == value {
                    result("OK", saved_revision, saved_value)
                } else {
                    result("IDEMPOTENCY_CONFLICT", revision, existing)
                },
                false,
            ));
        }
        tx.execute("INSERT INTO writes VALUES (?, ?)", params![key, value])?;
        let revision = tx.last_insert_rowid();
        tx.commit()?;
        Ok((result("OK", revision, value), true))
    }
}

fn current(db: &Connection) -> rusqlite::Result<(i64, i64)> {
    Ok(db
        .query_row(
            "SELECT rowid, value FROM writes ORDER BY rowid DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or_default())
}

fn result(code: &str, revision: i64, value: i64) -> Value {
    json!({"ok": code == "OK", "code": code, "revision": revision, "value": value})
}

impl ServerHandler for Probe {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("mom-mcp", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let output = json!({"type":"object","required":["ok","code","revision","value"],"properties":{
            "ok":{"type":"boolean"},"code":{"enum":["OK","INVALID_ARGUMENT","VALUE_OUT_OF_RANGE","IDEMPOTENCY_CONFLICT"]},
            "revision":{"type":"integer"},"value":{"type":"integer"}
        }});
        let tools = [
            (
                "get_probe",
                "Read the synthetic value and revision.",
                json!({"type":"object","additionalProperties":false}),
                true,
            ),
            (
                "set_probe",
                "Set a synthetic value from 0 to 100 with a stable key.",
                json!({"type":"object","additionalProperties":false,
                "properties":{"value":{"type":"integer"},"idempotency_key":{"type":"string"}}}),
                false,
            ),
        ]
        .map(|(name, description, input, read_only)| {
            let mut tool = Tool::new(name, description, input.as_object().unwrap().clone());
            tool.output_schema = Some(output.as_object().unwrap().clone().into());
            tool.annotations = Some(
                ToolAnnotations::new()
                    .read_only(read_only)
                    .destructive(false)
                    .idempotent(true),
            );
            tool
        });
        Ok(ListToolsResult::with_all_items(tools.into()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if !matches!(request.name.as_ref(), "get_probe" | "set_probe") {
            return Err(ErrorData::invalid_params("Unknown tool", None));
        }
        let probe = self.clone();
        let (value, inserted) = tokio::task::spawn_blocking(move || {
            probe.run(&request.name, request.arguments.unwrap_or_default())
        })
        .await
        .map_err(|_| ErrorData::internal_error("Probe worker failed", None))?
        .map_err(|_| ErrorData::internal_error("Probe database operation failed", None))?;
        if inserted && self.response_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.response_delay_ms)).await;
        }
        Ok(CallToolResult::structured(value).into())
    }
}
