use reqwest::{
    Response, StatusCode,
    header::{ACCEPT, CONTENT_TYPE},
};
use serde_json::{Value, json};

use super::{HttpOrderReader, ReadError, read_json};

const PROTOCOL_VERSION: &str = "2025-11-25";

impl HttpOrderReader<'_> {
    async fn send_carbon(&self, message: Value, initialized: bool) -> Result<Response, ReadError> {
        let mut request = self
            .client
            .post(
                self.origin
                    .join("/api/mcp")
                    .map_err(|_| ReadError::InvalidBinding)?,
            )
            .bearer_auth(self.source.token)
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&message);
        if initialized {
            request = request.header("MCP-Protocol-Version", PROTOCOL_VERSION);
        }
        let response = request
            .send()
            .await
            .map_err(|_| ReadError::SourceUnavailable)?;
        if response.status() == StatusCode::PAYMENT_REQUIRED {
            return Err(ReadError::EntitlementRequired);
        }
        if !response.status().is_success() {
            return Err(ReadError::SourceUnavailable);
        }
        Ok(response)
    }

    async fn carbon_call(&self, id: u64, method: &str, params: Value) -> Result<Value, ReadError> {
        let response = self
            .send_carbon(
                json!({"jsonrpc":"2.0", "id":id, "method": method, "params":params}),
                method != "initialize",
            )
            .await?;
        if response.status() != StatusCode::OK
            || response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|header| header.to_str().ok())
                .and_then(|value| value.split(';').next())
                .is_none_or(|mime| !mime.trim().eq_ignore_ascii_case("application/json"))
        {
            return Err(ReadError::InvalidResponse);
        }
        let mut message = read_json(response).await?;
        if message["jsonrpc"] != "2.0"
            || message["id"].as_u64() != Some(id)
            || message.get("error").is_some()
        {
            return Err(ReadError::InvalidResponse);
        }
        message
            .get_mut("result")
            .filter(|result| result.is_object())
            .map(Value::take)
            .ok_or(ReadError::InvalidResponse)
    }

    pub(super) async fn read_carbon(&self, source_id: &str) -> Result<Value, ReadError> {
        let initialized = self
            .carbon_call(
                1,
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "clientInfo": {"name": "mom-mcp", "version": env!("CARGO_PKG_VERSION")},
                    "capabilities": {}
                }),
            )
            .await?;
        if initialized["protocolVersion"] != PROTOCOL_VERSION {
            return Err(ReadError::InvalidResponse);
        }
        let notification = self
            .send_carbon(
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                true,
            )
            .await?;
        if notification.status() != StatusCode::ACCEPTED {
            return Err(ReadError::InvalidResponse);
        }

        let result = self
            .carbon_call(
                2,
                "tools/call",
                json!({
                    "name": "call_tool",
                    "arguments": {
                        "name": "production_getJob",
                        "arguments": {"id":source_id},
                        "fields": ["id", "jobId", "status", "updatedAt"]
                    }
                }),
            )
            .await?;
        match result.get("isError") {
            Some(Value::Bool(true)) => return Err(ReadError::UpstreamToolRefused),
            None | Some(Value::Bool(false)) => {}
            _ => return Err(ReadError::InvalidResponse),
        }
        let blocks = result["content"]
            .as_array()
            .ok_or(ReadError::InvalidResponse)?;
        let [block] = blocks.as_slice() else {
            return Err(ReadError::InvalidResponse);
        };
        if block["type"] != "text" {
            return Err(ReadError::InvalidResponse);
        }
        let text = block["text"].as_str().ok_or(ReadError::InvalidResponse)?;
        serde_json::from_str(text).map_err(|_| ReadError::InvalidResponse)
    }
}
