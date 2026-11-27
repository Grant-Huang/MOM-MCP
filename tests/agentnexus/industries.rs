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

    async fn versioned_write(&mut self, run: &Value, name: &str, mut args: Value) -> Value {
        let cursor = self.call("get_run", json!({"run_id":run})).await["cursor"].clone();
        args["expected_cursor"] = cursor.clone();
        args["idempotency_key"] = json!(format!("{name}-{cursor}"));
        self.business(run, name, args).await
    }

    async fn apply(&mut self, run: &Value, name: &str, args: Value) -> Value {
        let result = self.versioned_write(run, name, args).await;
        assert_eq!(result["ok"], true, "{name}: {result}");
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

async fn public_session(db: String) -> Session {
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
    Session {
        tools: connect(&[config]).await.unwrap(),
        trace: vec![],
    }
}

#[tokio::test]
async fn p6_agentnexus_three_industries() {
    let db = format!(
        "{}-industries.sqlite3",
        std::env::var("MOM_PROBE_DATABASE").unwrap()
    );
    let mut session = public_session(db).await;
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

#[tokio::test]
async fn p15_agentnexus_five_industry_session() {
    let db = format!(
        "{}-p15.sqlite3",
        std::env::var("MOM_PROBE_DATABASE").unwrap()
    );
    let mut session = public_session(db).await;
    let catalog = session.call("list_scenarios", json!({})).await;
    let selections = [
        ("cnc", 49, "set_dispatch_priority"),
        ("smt", 9, "smt_load"),
        ("assembly", 33, "set_dispatch_priority"),
        ("sheet-metal", 6, "create_sheet_nest"),
        ("wire-harness", 6, "cut_harness_batch"),
    ];
    let mut runs = Vec::new();
    for (scenario, version, tool) in selections {
        assert!(
            catalog["scenarios"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["scenario_id"] == scenario && item["version"] == version)
        );
        let opened = session
            .call(
                "open_scenario",
                json!({"scenario_id":scenario,"version":version}),
            )
            .await;
        assert_eq!(opened["ok"], true, "{opened}");
        let run = opened["run_id"].clone();
        assert!(!runs.contains(&run));
        let found = session
            .call("search_tools", json!({"run_id":run,"query":tool}))
            .await;
        assert!(
            found["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["name"] == tool)
        );
        let described = session
            .call("describe_tool", json!({"run_id":run,"name":tool}))
            .await;
        assert_eq!(described["tool"]["effect"], "write");
        runs.push(run);
    }
    let [cnc, smt, assembly, sheet, wire] = runs.as_slice() else {
        unreachable!()
    };
    for (run, order, priority) in [(cnc, "WO-1", 7), (assembly, "WO-E", 5)] {
        let request = json!({"work_order":order,"priority":priority,"expected_cursor":0,"idempotency_key":"shared"});
        let result = session
            .business(run, "set_dispatch_priority", request.clone())
            .await;
        assert_eq!(result["priority"], priority, "{result}");
        assert_eq!(
            session
                .business(run, "set_dispatch_priority", request)
                .await,
            result
        );
    }
    let before = session
        .business(
            smt,
            "smt_load",
            json!({"slot":"F1","reel":"REEL-P15","idempotency_key":"shared"}),
        )
        .await;
    assert_eq!(before["code"], "REEL_NOT_FOUND");
    receive_matrix_stock(&mut session, smt).await;
    let load = session
        .business(
            smt,
            "smt_load",
            json!({"slot":"F1","reel":"REEL-P15","idempotency_key":"shared"}),
        )
        .await;
    assert_eq!(load["load_id"], 1, "{load}");
    assert_eq!(session.business(sheet,"create_sheet_nest",json!({"nest_id":"N-P15","stock":"SHEET-FULL","work_orders":["WO-SHEET-A","WO-SHEET-C"],"idempotency_key":"shared"})).await["code"],"NEST_LAYOUT_UNAVAILABLE");
    let planned = session.business(sheet,"create_sheet_nest",json!({"nest_id":"N-P15","stock":"SHEET-FULL","work_orders":["WO-SHEET-A","WO-SHEET-B"],"idempotency_key":"shared"})).await;
    assert_eq!(
        planned["nest"]["placements"]["WO-SHEET-B"]["x_mm"], 30,
        "{planned}"
    );
    assert_eq!(
        session
            .business(
                sheet,
                "cut_sheet_nest",
                json!({"nest_id":"N-P15","machine":"LASER-A","idempotency_key":"cut"})
            )
            .await["code"],
        "MACHINE_NOT_CAPABLE"
    );
    let cut = session
        .business(
            sheet,
            "cut_sheet_nest",
            json!({"nest_id":"N-P15","machine":"LASER-B","idempotency_key":"cut"}),
        )
        .await;
    assert_eq!(cut["nest"]["product_mm2"], 1200, "{cut}");
    let batch = session.business(wire,"cut_harness_batch",json!({"batch_id":"C-P15","wire_lot":"WIRE-CU-1","serial_numbers":["WIRE-01"],"idempotency_key":"shared"})).await;
    assert_eq!(batch["batch"]["input_mm"], 660, "{batch}");
    assert_eq!(
        session
            .business(
                wire,
                "release_harness_serial",
                json!({"serial_number":"WIRE-01","idempotency_key":"early"})
            )
            .await["code"],
        "ENDPOINTS_INCOMPLETE"
    );
    assert_eq!(
        session
            .business(cnc, "get_harness_cut_batch", json!({"batch_id":"C-P15"}))
            .await["code"],
        "TOOL_UNAVAILABLE"
    );

    let trace = session
        .business(
            sheet,
            "trace_sheet_material",
            json!({"subject_type":"work_order","subject":"WO-SHEET-B"}),
        )
        .await;
    assert_eq!(trace["cuts"], json!([cut["nest"]]));
    verify_cnc_material_recovery(&mut session, cnc).await;
    verify_smt_feeder_recovery(&mut session, smt).await;
    verify_assembly_release_recovery(&mut session, assembly).await;
    verify_harness_release_recovery(&mut session, wire).await;

    for (run, cursor) in [(assembly, 16), (wire, 8), (cnc, 9), (sheet, 2), (smt, 10)] {
        let metadata = session.call("get_run", json!({"run_id":run})).await;
        assert_eq!(metadata["cursor"], cursor, "{metadata}");
        assert_eq!(metadata["clock"], 0);
        assert_eq!(
            session.call("close_run", json!({"run_id":run})).await["closed"],
            true
        );
    }

    let legacy = session
        .call(
            "open_scenario",
            json!({"scenario_id":"wire-harness","version":1}),
        )
        .await;
    assert_eq!(legacy["ok"], true);
    let legacy = &legacy["run_id"];
    assert_eq!(session.business(legacy,"cut_harness_batch",json!({"batch_id":"C-P15","wire_lot":"WIRE-CU-1","serial_numbers":["WIRE-01"],"idempotency_key":"shared"})).await["code"],"TOOL_UNAVAILABLE");
    let result = session
        .business(
            legacy,
            "harness_cut",
            json!({"serial":"WIRE-01","idempotency_key":"shared"}),
        )
        .await;
    assert_eq!(result["phase"], "cut", "{result}");
    assert_eq!(
        session.call("close_run", json!({"run_id":legacy})).await["closed"],
        true
    );

    if let Some(path) = std::env::var_os("MOM_P15_SESSION_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&session.trace).unwrap()).unwrap();
    }
}

async fn receive_matrix_stock(session: &mut Session, run: &Value) {
    for (name, args) in [
        ("create_item", json!({"item_id":"PART","base_unit":"each"})),
        (
            "create_item_revision",
            json!({"item_id":"PART","revision":1,"name":"Part"}),
        ),
        (
            "activate_revision",
            json!({"item_id":"PART","revision":1,"revision_type":"item"}),
        ),
        (
            "create_receipt",
            json!({"receipt_id":"R-P15","item_id":"PART","revision":1,"quantity":"3","requires_inspection":false}),
        ),
        (
            "assign_receipt_tracking",
            json!({"receipt_id":"R-P15","lot_id":"REEL-P15","supplier_lot":"S-P15","location":"RAW-02"}),
        ),
        ("post_receipt", json!({"receipt_id":"R-P15"})),
    ] {
        session.apply(run, name, args).await;
    }
}

async fn verify_cnc_material_recovery(session: &mut Session, run: &Value) {
    let lot = json!({"lot_id":"REEL-P15"});
    assert_eq!(
        session.business(run, "get_lot", lot.clone()).await["code"],
        "LOT_NOT_FOUND"
    );
    receive_matrix_stock(session, run).await;
    let ready = session.business(run, "get_lot", lot.clone()).await;
    assert_eq!(ready["lot"]["availability"], "available");
    assert_eq!(ready["lot"]["quantity"], "3");
    session
        .apply(
            run,
            "hold_stock_lot",
            json!({"lot_id":"REEL-P15","hold_id":"H-P15","reason":"supplier review"}),
        )
        .await;
    assert_eq!(
        session.business(run, "get_lot", lot.clone()).await["lot"]["availability"],
        "held"
    );
    session
        .apply(
            run,
            "release_stock_lot_hold",
            json!({"hold_id":"H-P15","reason":"supplier lot accepted"}),
        )
        .await;
    let recovered = session.business(run, "get_lot", lot).await;
    assert_eq!(
        recovered["lot"]["availability"],
        ready["lot"]["availability"]
    );
    assert_eq!(recovered["lot"]["quantity"], ready["lot"]["quantity"]);
}

async fn verify_smt_feeder_recovery(session: &mut Session, run: &Value) {
    let request = json!({"slot":"F2","reel":"REEL-P15","idempotency_key":"second-feeder"});
    session
        .apply(
            run,
            "hold_stock_lot",
            json!({"lot_id":"REEL-P15","hold_id":"H-P15","reason":"supplier review"}),
        )
        .await;
    assert_eq!(
        session.business(run, "smt_load", request.clone()).await["code"],
        "MATERIAL_HELD"
    );
    session
        .apply(
            run,
            "release_stock_lot_hold",
            json!({"hold_id":"H-P15","reason":"supplier lot accepted"}),
        )
        .await;
    let result = session.business(run, "smt_load", request).await;
    assert_eq!(result["load_id"], 2, "{result}");
}

async fn verify_assembly_release_recovery(session: &mut Session, run: &Value) {
    for item in ["FRAME", "BASE"] {
        session
            .apply(
                run,
                "create_item",
                json!({"item_id":item,"base_unit":"each"}),
            )
            .await;
        session
            .apply(
                run,
                "create_item_revision",
                json!({"item_id":item,"revision":1,"name":item}),
            )
            .await;
        session
            .apply(
                run,
                "activate_revision",
                json!({"item_id":item,"revision":1,"revision_type":"item"}),
            )
            .await;
    }
    for (tool, args) in [
        (
            "create_bom_revision",
            json!({"item_id":"FRAME","revision":1}),
        ),
        (
            "set_bom_components",
            json!({"item_id":"FRAME","revision":1,"components":{"BASE":{"revision":1,"quantity":1,"unit":"each"}}}),
        ),
        (
            "activate_revision",
            json!({"item_id":"FRAME","revision":1,"revision_type":"bom"}),
        ),
        (
            "create_route_revision",
            json!({"item_id":"FRAME","revision":1}),
        ),
        (
            "set_route_operations",
            json!({"item_id":"FRAME","revision":1,"operations":{"build":{"work_center":"CELL-A","predecessors":[]}}}),
        ),
        (
            "activate_revision",
            json!({"item_id":"FRAME","revision":1,"revision_type":"route"}),
        ),
        (
            "create_inspection_plan_revision",
            json!({"plan_id":"QA","revision":1,"characteristics":{"width":{"unit":"mm","lower_limit":1,"upper_limit":2}}}),
        ),
        (
            "activate_inspection_plan_revision",
            json!({"plan_id":"QA","revision":1}),
        ),
    ] {
        session.apply(run, tool, args).await;
    }
    let mut request = json!({"work_order":"WO-E","definition_selection":{"item_id":"FRAME","inspection_plan_id":"QA"}});
    let ready = session
        .business(run, "get_release_readiness", request.clone())
        .await;
    assert_eq!(ready["ready"], true, "{ready}");
    request["snapshot_fingerprint"] = json!("0".repeat(64));
    assert_eq!(
        session
            .versioned_write(run, "release_work_order", request.clone())
            .await["code"],
        "SNAPSHOT_CHANGED"
    );
    request["snapshot_fingerprint"] = ready["definition_snapshot"]["fingerprint"].clone();
    session.apply(run, "release_work_order", request).await;
    let snapshot = session
        .business(run, "get_order_snapshot", json!({"work_order":"WO-E"}))
        .await;
    assert_eq!(snapshot["snapshot"], ready["definition_snapshot"]);
}

async fn verify_harness_release_recovery(session: &mut Session, run: &Value) {
    for (segment, end, lot, tool) in [
        ("POWER", "A", "TERM-PWR-1", "TOOL-PWR-1"),
        ("POWER", "B", "TERM-PWR-ALT", "TOOL-PWR-1"),
        ("SIGNAL", "A", "TERM-SIG-1", "TOOL-SIG-1"),
        ("SIGNAL", "B", "TERM-SIG-1", "TOOL-SIG-1"),
    ] {
        let result = session
            .business(
                run,
                "crimp_harness_endpoint",
                json!({
                    "serial_number":"WIRE-01","segment_id":segment,"end":end,"terminal_lot":lot,
                    "tooling_id":tool,"idempotency_key":format!("{segment}-{end}")
                }),
            )
            .await;
        assert_eq!(result["ok"], true, "{result}");
    }
    let release = json!({"serial_number":"WIRE-01","idempotency_key":"early"});
    for (result, evidence) in [("fail", "E-FAIL"), ("pass", "E-PASS")] {
        let recorded = session.business(run,"record_harness_continuity_test",json!({
            "serial_number":"WIRE-01","revision":1,"result":result,"evidence":evidence,"idempotency_key":evidence
        })).await;
        assert_eq!(recorded["test"]["result"], result, "{recorded}");
        if result == "fail" {
            assert_eq!(
                session
                    .business(run, "release_harness_serial", release.clone())
                    .await["code"],
                "TEST_PASS_REQUIRED"
            );
        }
    }
    let released = session
        .business(run, "release_harness_serial", release)
        .await;
    assert_eq!(released["released"], true, "{released}");
    let status = session
        .business(
            run,
            "get_harness_crimp_status",
            json!({"serial_number":"WIRE-01"}),
        )
        .await;
    assert_eq!(status["disposition"]["tests"].as_array().unwrap().len(), 2);
    assert_eq!(status["disposition"]["released"], true);
}
