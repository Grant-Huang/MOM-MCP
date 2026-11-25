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
                json!({"scenario_id":scenario,"version":2}),
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

#[tokio::test]
async fn p8c_agentnexus_cnc_des_recovery() {
    let db = format!("{}-recovery.sqlite3", std::env::var("MOM_PROBE_DATABASE").unwrap());
    let bindings = |names: &[&str]| names.iter().map(|name| json!({
        "name":name,"effect":"workspace_write","description":name,"capability_id":format!("mom.recovery.{name}")
    })).collect::<Vec<_>>();
    let connect_side = async |control: bool, names: &[&str]| {
        let mut args = vec!["stdio","--scenario-tools","--db",&db];
        if control { args.push("--control-tools"); }
        let config: McpServerConfig = serde_json::from_value(json!({
            "id":"mom","transport":"stdio","command":std::env::var("MOM_PROBE_BINARY").unwrap(),
            "args":args,"timeout_ms":5000,"tools":bindings(names)
        })).unwrap();
        Session { tools: connect(&[config]).await.unwrap(), trace: vec![] }
    };
    let mut agent = connect_side(false, &["open_scenario","read_events","call_tool"]).await;
    let mut clock = connect_side(true, &["advance_run","inspect_run"]).await;
    let opened = agent.call("open_scenario",json!({"scenario_id":"cnc","version":15})).await;
    assert_eq!(opened["ok"],true);
    let run=&opened["run_id"];
    for (tool,args) in [
        ("release_work_order",json!({"work_order":"WO-1","expected_cursor":0,"idempotency_key":"release"})),
        ("dispatch_operation",json!({"work_order":"WO-1","operation":"machining","work_center":"MACH-01","expected_cursor":1,"idempotency_key":"dispatch"})),
        ("issue_material",json!({"work_order":"WO-1","batch":"LOT-1","idempotency_key":"issue"})),
        ("start_operation",json!({"work_order":"WO-1","operation":"machining","expected_cursor":3,"idempotency_key":"start"})),
    ] {
        assert_eq!(agent.business(run,tool,args).await["ok"],true);
    }
    assert_eq!(clock.call("advance_run",json!({"run_id":run,"max_events":3})).await["ok"],true);
    let events=agent.call("read_events",json!({"run_id":run})).await;
    assert!(events["events"].as_array().unwrap().iter().any(|event|event["kind"]=="cnc_equipment_stopped"));
    let affected=agent.business(run,"get_resource_capability",json!({"work_center":"MACH-01"})).await;
    assert_eq!(affected["resource"]["occupied_by"],"WO-1");
    assert_eq!(affected["resource"]["available"],false);
    let stop=agent.business(run,"mes_status",json!({"work_order":"WO-1"})).await;
    assert_eq!(stop["blocking"],"EQUIPMENT_UNAVAILABLE");
    let paused=agent.business(run,"pause_operation",json!({"work_order":"WO-1","operation":"machining","reason":"equipment stopped","expected_cursor":stop["cursor"],"idempotency_key":"pause"})).await;
    assert_eq!(paused["ok"],true);
    let receipt=agent.business(run,"request_maintenance",json!({"asset":"CNC-1","expected_cursor":paused["cursor"],"idempotency_key":"repair"})).await;
    assert_eq!(receipt["status"],"maintenance_requested");
    assert_eq!(clock.call("advance_run",json!({"run_id":run,"max_events":2})).await["ok"],true);
    let ready=agent.business(run,"mes_status",json!({"work_order":"WO-1"})).await;
    assert_eq!(ready["equipment_down"],false);
    let resumed=agent.business(run,"resume_operation",json!({"work_order":"WO-1","operation":"machining","expected_cursor":ready["cursor"],"idempotency_key":"resume"})).await;
    assert_eq!(resumed["ok"],true);
    let report=agent.business(run,"post_operation_report",json!({"work_order":"WO-1","operation":"machining","quantity":1,"expected_cursor":resumed["cursor"],"idempotency_key":"report"})).await;
    assert_eq!(report["remaining"],9,"{report}");
    assert_eq!(clock.call("inspect_run",json!({"run_id":run})).await["replay_verified"],true);
    if let Ok(output) = std::env::var("MOM_RECOVERY_REPORT") {
        std::fs::write(output, serde_json::to_vec_pretty(&json!({"agent_trace":agent.trace,"replay_verified":true})).unwrap()).unwrap();
    }
}

fn rebind_run_values(value: &mut Value, bindings: &std::collections::BTreeMap<String, String>) {
    match value {
        Value::Object(fields) => fields.values_mut().for_each(|value| rebind_run_values(value, bindings)),
        Value::Array(items) => items.iter_mut().for_each(|value| rebind_run_values(value, bindings)),
        Value::String(text) => {
            if let Some(rebound) = bindings.get(text) {
                *text = rebound.clone();
            }
        }
        _ => {}
    }
}

#[tokio::test]
async fn p9d_agentnexus_cnc_closure() {
    let input = std::env::var("MOM_P9D_CNC_INPUT").unwrap();
    let evidence: Value = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    let original_run = &evidence["run_id"];
    let calls = evidence["calls"].as_array().unwrap();
    let (open, calls) = calls.split_first().unwrap();
    assert_eq!(open["tool"], "open_scenario");
    assert_eq!(open["arguments"]["scenario_id"], "cnc");
    assert_eq!(open["arguments"]["version"], 45);

    let bindings = [
        ("open_scenario", "workspace_write"),
        ("get_run", "read_only"),
        ("call_tool", "workspace_write"),
    ].map(|(name, effect)| json!({
        "name":name,"effect":effect,"description":name,"capability_id":format!("mom.closure.{name}")
    }));
    let db = format!("{}-closure.sqlite3", std::env::var("MOM_PROBE_DATABASE").unwrap());
    let config: McpServerConfig = serde_json::from_value(json!({
        "id":"mom","transport":"stdio","command":std::env::var("MOM_PROBE_BINARY").unwrap(),
        "args":["stdio","--scenario-tools","--db",db],"timeout_ms":5000,"tools":bindings
    })).unwrap();
    let mut session = Session { tools: connect(&[config]).await.unwrap(), trace: vec![] };
    let opened = session.call("open_scenario", open["arguments"].clone()).await;
    assert_eq!(opened["ok"], true, "{opened}");
    let run = &opened["run_id"];
    assert!(run.is_string() && run != original_run);
    let mut run_values = std::collections::BTreeMap::from([(
        original_run.as_str().unwrap().to_owned(), run.as_str().unwrap().to_owned(),
    )]);
    for entry in calls {
        let tool = entry["tool"].as_str().unwrap();
        assert!(matches!(tool, "get_run" | "call_tool"), "{tool}");
        let mut arguments = entry["arguments"].clone();
        assert_eq!(arguments["run_id"], *original_run);
        rebind_run_values(&mut arguments, &run_values);
        let actual = session.call(tool, arguments).await;
        let mut expected = entry["response"].clone();
        if entry["arguments"]["name"] == "get_release_readiness" && expected["ready"] == true {
            let fingerprint = |response: &Value| response["definition_snapshot"]["fingerprint"].as_str().unwrap().to_owned();
            let original = fingerprint(&expected);
            let current = fingerprint(&actual);
            if let Some(previous) = run_values.insert(original, current.clone()) {
                assert_eq!(previous, current);
            }
        }
        rebind_run_values(&mut expected, &run_values);
        assert_eq!(actual, expected, "{tool} {}", entry["arguments"]["name"]);
    }
    let final_state = session.business(run, "reconcile_work_order", json!({"work_order":"WO-1"})).await;
    assert_eq!(final_state["balanced"], true);
    let settlement = &final_state["settlement"];
    assert_eq!(settlement["closed"], true);
    assert_eq!(settlement["differences"], json!([]));
    assert_eq!(settlement["blocking"], json!([]));
    assert_eq!(settlement["production"]["received"], 10);
    assert_eq!(settlement["production"]["wip_open"], 0);
    let output = std::env::var("MOM_P9D_AGENT_REPORT").unwrap();
    std::fs::write(output, serde_json::to_vec_pretty(&json!({
        "run_id":run,"agent_trace":session.trace,"settlement":settlement
    })).unwrap()).unwrap();
}
