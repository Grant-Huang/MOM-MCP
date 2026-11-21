use std::{path::PathBuf, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
};
use rmcp::{RoleClient, ServiceExt, model::CallToolRequestParams, service::RunningService};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::{net::TcpListener, process::Command};

#[derive(Clone)]
struct Bridge {
    db: PathBuf,
    executable: PathBuf,
    control_tools: bool,
}

type Reply = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn unavailable() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({"ok":false,"code":"MCP_UNAVAILABLE"})),
    )
}

impl Bridge {
    async fn connect(&self, scenarios: bool) -> anyhow::Result<RunningService<RoleClient, ()>> {
        let mut command = Command::new(&self.executable);
        command.arg("stdio").arg("--db").arg(&self.db);
        if scenarios {
            command.arg("--scenario-tools");
        }
        if scenarios && self.control_tools {
            command.arg("--control-tools");
        }
        let transport = rmcp::transport::TokioChildProcess::new(command)?;
        Ok(().serve(transport).await?)
    }
}

async fn list(State(bridge): State<Bridge>) -> Reply {
    tokio::time::timeout(Duration::from_secs(5), async {
        let client = bridge.connect(false).await?;
        let output = client.peer().list_tools(None).await?;
        client.cancel().await?;
        Ok::<_, anyhow::Error>(Json(serde_json::to_value(output)?))
    })
    .await
    .map_err(|_| unavailable())?
    .map_err(|_| unavailable())
}

#[derive(Deserialize)]
struct Call {
    name: String,
    arguments: Map<String, Value>,
}

async fn call(State(bridge): State<Bridge>, Json(request): Json<Call>) -> Reply {
    if !matches!(request.name.as_str(), "get_probe" | "set_probe") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"ok":false,"code":"UNKNOWN_TOOL"})),
        ));
    }
    forward(bridge, request, false).await
}

async fn scenarios(State(bridge): State<Bridge>, Json(request): Json<Call>) -> Reply {
    if !crate::catalog::gateway(&request.name)
        .is_some_and(|tool| bridge.control_tools || !tool.control)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"ok":false,"code":"UNKNOWN_TOOL"})),
        ));
    }
    forward(bridge, request, true).await
}

async fn forward(bridge: Bridge, request: Call, scenarios: bool) -> Reply {
    tokio::time::timeout(Duration::from_secs(5), async {
        let client = bridge.connect(scenarios).await?;
        let response = client
            .peer()
            .call_tool(CallToolRequestParams::new(request.name).with_arguments(request.arguments))
            .await?;
        client.cancel().await?;
        let value = response
            .structured_content
            .ok_or_else(|| anyhow::anyhow!("missing structured result"))?;
        Ok::<_, anyhow::Error>(Json(value))
    })
    .await
    .map_err(|_| unavailable())?
    .map_err(|_| unavailable())
}

async fn document(Path(path): Path<String>) -> impl IntoResponse {
    let text = match path.as_str() {
        "DESIGN.md" => include_str!("../docs/DESIGN.md"),
        "FEATURES.md" => include_str!("../docs/FEATURES.md"),
        "IMPLEMENTATION_PLAN.md" => include_str!("../docs/IMPLEMENTATION_PLAN.md"),
        "TEST_AND_MOCK_PLAN.md" => include_str!("../docs/TEST_AND_MOCK_PLAN.md"),
        "feature-map.toml" => include_str!("../docs/feature-map.toml"),
        "acceptance/README.md" => include_str!("../docs/acceptance/README.md"),
        "acceptance/P0.md" => include_str!("../docs/acceptance/P0.md"),
        "acceptance/P1.md" => include_str!("../docs/acceptance/P1.md"),
        "acceptance/P2.md" => include_str!("../docs/acceptance/P2.md"),
        "acceptance/P3.md" => include_str!("../docs/acceptance/P3.md"),
        "acceptance/P4.md" => include_str!("../docs/acceptance/P4.md"),
        "acceptance/P5.md" => include_str!("../docs/acceptance/P5.md"),
        "acceptance/P6.md" => include_str!("../docs/acceptance/P6.md"),
        "acceptance/P7.md" => include_str!("../docs/acceptance/P7.md"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([("content-type", "text/plain; charset=utf-8")], text).into_response()
}

pub async fn serve(db: PathBuf, port: u16, control_tools: bool) -> anyhow::Result<()> {
    let state = Bridge {
        db,
        control_tools,
        executable: std::env::current_exe()?,
    };
    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../index.html")) }))
        .route(
            "/acceptance",
            get(|| async { Html(include_str!("../web/acceptance.html")) }),
        )
        .route(
            "/acceptance.js",
            get(|| async {
                (
                    [("content-type", "text/javascript; charset=utf-8")],
                    include_str!("../web/acceptance.js"),
                )
                    .into_response()
            }),
        )
        .route("/docs/{*path}", get(document))
        .route(
            "/README.md",
            get(|| async {
                (
                    [("content-type", "text/plain; charset=utf-8")],
                    include_str!("../README.md"),
                )
            }),
        )
        .route(
            "/config/agentnexus.p0.toml",
            get(|| async {
                (
                    [("content-type", "text/plain; charset=utf-8")],
                    include_str!("../config/agentnexus.p0.toml"),
                )
            }),
        )
        .route(
            "/scenarios",
            get(|| async { Html(include_str!("../web/scenarios.html")) }),
        )
        .route(
            "/scenarios.js",
            get(|| async {
                (
                    [("content-type", "text/javascript; charset=utf-8")],
                    include_str!("../web/scenarios.js"),
                )
            }),
        )
        .route("/api/scenarios", post(scenarios))
        .route(
            "/api/control-enabled",
            get(|State(bridge): State<Bridge>| async move { Json(bridge.control_tools) }),
        )
        .route("/api/tools", get(list))
        .route("/api/call", post(call))
        .with_state(state);
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    println!(
        "Serving acceptance environment at http://{}",
        listener.local_addr()?
    );
    axum::serve(listener, app).await?;
    Ok(())
}
