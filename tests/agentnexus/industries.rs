use an_agents::{
    ToolRegistry,
    mcp::{McpServerConfig, connect},
};
use serde_json::{Value, json};

struct Session {
    tools: ToolRegistry,
    trace: Vec<Value>,
}

impl Session {
    async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let output = self
            .tools
            .call(&format!("mom.{tool}"), &arguments)
            .await
            .unwrap();
        let response: Value = serde_json::from_str(&output.text).unwrap();
        assert_eq!(response["status"], "success");
        let result = response["data"]["structuredContent"].clone();
        self.trace
            .push(json!({"tool":tool,"arguments":arguments,"result":result}));
        result
    }

    async fn business(&mut self, run: &Value, name: &str, arguments: Value) -> Value {
        self.call(
            "call_tool",
            json!({"run_id":run,"name":name,"arguments":arguments}),
        )
        .await
    }
}

#[tokio::test]
async fn p6_agentnexus_three_industries() {
    let db = format!(
        "{}-industries.sqlite3",
        std::env::var("MOM_PROBE_DATABASE").unwrap()
    );
    let bindings = [
        ("list_scenarios","read_only"),("open_scenario","workspace_write"),("get_run","read_only"),
        ("close_run","workspace_write"),("search_tools","read_only"),("describe_tool","read_only"),
        ("call_tool","workspace_write"),("read_events","read_only"),
    ].map(|(name,effect)| json!({"name":name,"effect":effect,"description":name,"capability_id":format!("mom.release.{name}")}));
    let config: McpServerConfig = serde_json::from_value(json!({
        "id":"mom","transport":"stdio","command":std::env::var("MOM_PROBE_BINARY").unwrap(),
        "args":["stdio","--scenario-tools","--db",db],"timeout_ms":5000,"tools":bindings
    }))
    .unwrap();
    let mut session = Session {
        tools: connect(&[config]).await.unwrap(),
        trace: vec![],
    };
    let scenarios = session.call("list_scenarios", json!({})).await;
    let mut runs = Vec::new();
    for scenario in ["cnc", "smt", "assembly"] {
        assert!(
            scenarios["scenarios"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["scenario_id"] == scenario && item["version"] == 2)
        );
        let opened = session
            .call(
                "open_scenario",
                json!({"scenario_id":scenario,"version":2,"seed":42}),
            )
            .await;
        assert_eq!(opened["ok"], true);
        assert!(!runs.contains(&opened["run_id"]));
        runs.push(opened["run_id"].clone());
    }
    let [cnc, smt, assembly] = runs.as_slice() else {
        unreachable!()
    };
    for (run, tool) in [
        (cnc, "hold_material"),
        (assembly, "assembly_priority"),
        (smt, "smt_release"),
    ] {
        let found = session
            .call("search_tools", json!({"run_id":run,"query":tool}))
            .await;
        assert_eq!(found["tools"][0]["name"], tool);
        let described = session
            .call("describe_tool", json!({"run_id":run,"name":tool}))
            .await;
        assert_eq!(described["tool"]["effect"], "write");
    }

    assert_eq!(
        session
            .business(cnc, "mes_status", json!({"work_order":"WO-1"}))
            .await["on_hand"],
        10
    );
    assert_eq!(
        session
            .business(smt, "smt_status", json!({"work_order":"WO-A"}))
            .await["released"],
        false
    );
    assert_eq!(
        session
            .business(assembly, "assembly_kit", json!({"work_order":"WO-E"}))
            .await["ready"],
        true
    );
    assert_eq!(
        session
            .business(
                cnc,
                "hold_material",
                json!({"batch":"LOT-1","reason":"baseline","idempotency_key":"shared-key"})
            )
            .await["held"],
        true
    );
    assert_eq!(
        session
            .business(
                assembly,
                "assembly_priority",
                json!({"work_order":"WO-E","priority":4,"idempotency_key":"shared-key"})
            )
            .await["state"]["priority"],
        4
    );
    assert_eq!(
        session
            .business(
                smt,
                "smt_release",
                json!({"work_order":"WO-A","idempotency_key":"shared-key"})
            )
            .await["ok"],
        true
    );
    assert_eq!(
        session
            .business(smt, "smt_status", json!({"work_order":"WO-A"}))
            .await["released"],
        true
    );
    assert_eq!(
        session
            .business(cnc, "mes_status", json!({"work_order":"WO-1"}))
            .await["blocking"],
        "QUALITY_HOLD"
    );
    assert_eq!(
        session
            .business(assembly, "assembly_kit", json!({"work_order":"WO-E"}))
            .await["stock"]["FRAME"],
        2
    );
    assert_eq!(
        session
            .business(smt, "assembly_kit", json!({"work_order":"WO-E"}))
            .await["code"],
        "TOOL_UNAVAILABLE"
    );

    for run in &runs {
        let metadata = session.call("get_run", json!({"run_id":run})).await;
        assert_eq!(metadata["clock"], 0);
        assert_eq!(metadata["cursor"], 1);
        let events = session.call("read_events", json!({"run_id":run})).await;
        assert_eq!(events["events"], json!([]));
        assert_eq!(
            session.call("close_run", json!({"run_id":run})).await["closed"],
            true
        );
    }
    if let Some(path) = std::env::var_os("MOM_INDUSTRY_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&session.trace).unwrap()).unwrap();
    }
}
