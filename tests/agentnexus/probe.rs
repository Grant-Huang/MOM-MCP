use an_agents::{
    ToolRegistry,
    mcp::{McpServerConfig, connect},
};
use serde_json::{Value, json};

fn config(delay: u32, timeout: u32, db: &str) -> McpServerConfig {
    serde_json::from_value(json!({
        "id": "mom_probe",
        "transport": "stdio",
        "command": std::env::var("MOM_PROBE_BINARY").unwrap(),
        "args": ["stdio", "--db", db, "--response-delay-ms", &delay.to_string()],
        "timeout_ms": timeout,
        "tools": [
            {"name":"get_probe","capability_id":"mom.probe.read","description":"Read the probe","effect":"read_only"},
            {"name":"set_probe","capability_id":"mom.probe.write","description":"Write the probe","effect":"workspace_write"}
        ]
    })).unwrap()
}

async fn call(tools: &ToolRegistry, name: &str, args: Value) -> Value {
    let result = tools.call(name, &args).await.unwrap();
    let envelope: Value = serde_json::from_str(&result.text).unwrap();
    assert_eq!(envelope["status"], "success");
    envelope["data"]["structuredContent"].clone()
}

#[tokio::test]
async fn p0_agentnexus_discovery_read_and_write() {
    let db = std::env::var("MOM_PROBE_DATABASE").unwrap();
    let tools = connect(&[config(0, 5000, &db)]).await.unwrap();
    assert_eq!(
        tools.names(),
        ["mom_probe.get_probe", "mom_probe.set_probe"]
    );
    assert_eq!(
        call(&tools, "mom_probe.get_probe", json!({})).await["revision"],
        0
    );
    let request = json!({"value":42,"idempotency_key":"agent-call"});
    let expected = json!({"ok":true,"code":"OK","value":42,"revision":1});
    assert_eq!(
        call(&tools, "mom_probe.set_probe", request.clone()).await,
        expected
    );
    assert_eq!(call(&tools, "mom_probe.set_probe", request).await, expected);
    assert_eq!(
        call(&tools, "mom_probe.set_probe", json!({})).await["code"],
        "INVALID_ARGUMENT"
    );
    assert_eq!(
        call(
            &tools,
            "mom_probe.set_probe",
            json!({"value":101,"idempotency_key":"range"})
        )
        .await["code"],
        "VALUE_OUT_OF_RANGE"
    );
    assert_eq!(
        call(&tools, "mom_probe.get_probe", json!({})).await,
        expected
    );
}

#[tokio::test]
async fn p0_agentnexus_timeout_and_reconnect() {
    let base = std::env::var("MOM_PROBE_DATABASE").unwrap();
    let db = format!("{base}-timeout.sqlite3");
    let tools = connect(&[config(30_000, 2000, &db)]).await.unwrap();
    let args = json!({"value":21,"idempotency_key":"timeout"});
    let failure = tools.call("mom_probe.set_probe", &args).await.unwrap_err();
    assert!(failure.to_string().contains("E_MCP_TIMEOUT"), "{failure}");
    assert!(tools.call("mom_probe.get_probe", &json!({})).await.is_err());
    drop(tools);
    let reconnected = connect(&[config(0, 5000, &db)]).await.unwrap();
    let state = call(&reconnected, "mom_probe.get_probe", json!({})).await;
    assert_eq!(state["value"], 21);
    assert_eq!(state["revision"], 1);
    assert_eq!(call(&reconnected, "mom_probe.set_probe", args).await, state);
}

#[tokio::test]
async fn p0_agentnexus_agent_uses_probe() {
    use an_core::{AgentAdapter, EventPublisher, EventSink, TaskEvent, TaskSpec, WorkerStatus};
    use axum::{Json, Router, routing::post};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct Noop;
    impl EventPublisher for Noop {
        fn publish(&self, _: TaskEvent) {}
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let app = Router::new().route("/v1/chat/completions", post(move |Json(req): Json<Value>| {
        let seen = seen.clone();
        async move {
            let observed = |fragment: &str| {
                req["messages"].as_array().unwrap().iter().any(|m| {
                    m["content"].as_str().unwrap_or_default().contains(fragment)
                })
            };
            let step = match seen.fetch_add(1, Ordering::SeqCst) {
                0 => json!({"thought":"Read probe","action":"tool_call","tool":"mom_probe.get_probe","args":{},"answer":""}),
                1 => {
                    assert!(observed(r#""revision":0"#));
                    json!({"thought":"Update probe","action":"tool_call","tool":"mom_probe.set_probe","args":{"value":42,"idempotency_key":"general"},"answer":""})
                }
                2 => {
                    assert!(observed(r#""revision":1"#) && observed(r#""value":42"#));
                    json!({"thought":"Done","action":"final","tool":"","args":{},"answer":"Probe updated"})
                }
                _ => panic!("unexpected model call"),
            };
            ([(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices":[{"delta":{"content":step.to_string()},"finish_reason":"stop"}]})))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let llm = an_llm::LlmClient::new(
        serde_json::from_value(json!({"base_url":base,"model":"mock"})).unwrap(),
    )
    .unwrap();
    let db = format!(
        "{}-general.sqlite3",
        std::env::var("MOM_PROBE_DATABASE").unwrap()
    );
    let tools = connect(&[config(0, 5000, &db)]).await.unwrap();
    let agent = an_agents::GeneralAgent::new(llm, tools, 4, 2);
    let task: TaskSpec = serde_json::from_value(json!({
        "task_id":"mom-task","conversation_id":"c","goal":"Update the probe","context":"",
        "route":{"task_class":"L2","execution_role":"general",
            "required_tools":["mom_probe.get_probe","mom_probe.set_probe"],"risk_level":"low","reason":"test"}
    })).unwrap();
    let result = agent
        .run(task, EventSink::new(Arc::new(Noop), "c", "mom-task"))
        .await
        .unwrap();
    assert_eq!(result.status, WorkerStatus::Succeeded);
    assert_eq!(result.summary, "Probe updated");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    server.abort();
}
