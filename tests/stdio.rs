use std::{path::Path, process::Stdio, time::Duration};

use rmcp::{RoleClient, ServiceExt, model::CallToolRequestParams, service::RunningService};
use serde_json::{Value, json};
use tokio::{
    process::{Child, Command},
    time::timeout,
};

const WAIT: Duration = Duration::from_secs(5);

async fn connect(db: &Path, delay: &str) -> (RunningService<RoleClient, ()>, Child) {
    connect_mode(db, delay, false).await
}

async fn connect_mode(
    db: &Path,
    delay: &str,
    scenarios: bool,
) -> (RunningService<RoleClient, ()>, Child) {
    connect_options(db, delay, scenarios, false).await
}

async fn connect_options(
    db: &Path,
    delay: &str,
    scenarios: bool,
    control: bool,
) -> (RunningService<RoleClient, ()>, Child) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mom-mcp"));
    command
        .args(["stdio", "--db"])
        .arg(db)
        .args(["--response-delay-ms", delay]);
    if scenarios {
        command.arg("--scenario-tools");
    }
    if control {
        command.arg("--control-tools");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let transport = rmcp::transport::async_rw::AsyncRwTransport::new(
        child.stdout.take().unwrap(),
        child.stdin.take().unwrap(),
    );
    (
        timeout(WAIT, ().serve(transport)).await.unwrap().unwrap(),
        child,
    )
}

async fn call(client: &RunningService<RoleClient, ()>, name: &str, args: Value) -> Value {
    let action = args.get("name").and_then(Value::as_str);
    let request = CallToolRequestParams::new(name.to_owned())
        .with_arguments(args.as_object().unwrap().clone());
    let response = timeout(WAIT, client.peer().call_tool(request))
        .await
        .unwrap_or_else(|_| panic!("MCP request timed out after {WAIT:?}: {name} {action:?}"))
        .unwrap();
    assert_ne!(response.is_error, Some(true));
    response.structured_content.unwrap()
}

#[tokio::test]
async fn p0_01_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let (client, mut child) = connect(&dir.path().join("probe.sqlite3"), "0").await;
    let result = timeout(WAIT, client.peer().list_tools(None))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        result
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        ["get_probe", "set_probe"]
    );
    assert!(result.tools.iter().all(|tool| tool.output_schema.is_some()));
    assert_eq!(
        timeout(WAIT, call(&client, "get_probe", json!({})))
            .await
            .unwrap(),
        json!({"ok":true,"code":"OK","revision":0,"value":0})
    );
    timeout(WAIT, client.cancel()).await.unwrap().unwrap();
    assert!(
        timeout(WAIT, child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn p0_02_write_and_rejections() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect(&dir.path().join("probe.sqlite3"), "0").await;
    timeout(WAIT, async {
        let write = json!({"value":42,"idempotency_key":"first"});
        let expected = json!({"ok":true,"code":"OK","revision":1,"value":42});
        assert_eq!(call(&client, "set_probe", write.clone()).await, expected);
        assert_eq!(call(&client, "set_probe", write).await, expected);
        for (args, code) in [
            (json!({}), "INVALID_ARGUMENT"),
            (
                json!({"value":101,"idempotency_key":"second"}),
                "VALUE_OUT_OF_RANGE",
            ),
            (
                json!({"value":43,"idempotency_key":"first"}),
                "IDEMPOTENCY_CONFLICT",
            ),
        ] {
            assert_eq!(
                call(&client, "set_probe", args).await,
                json!({"ok":false,"code":code,"revision":1,"value":42})
            );
        }
        assert_eq!(call(&client, "get_probe", json!({})).await, expected);
        client.cancel().await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p0_03_timeout_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("probe.sqlite3");
    let (client, mut child) = connect(&db, "60000").await;
    let args = json!({"value":21,"idempotency_key":"lost"});
    let mut pending = Box::pin(timeout(WAIT, call(&client, "set_probe", args.clone())));
    // Observe the durable commit independently before dropping the delayed response.
    let observer = async {
        loop {
            let connection = rusqlite::Connection::open(&db).unwrap();
            let count: i64 = connection
                .query_row("SELECT count(*) FROM writes", [], |r| r.get(0))
                .unwrap();
            if count == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::select! {
        observed = timeout(WAIT, observer) => observed.expect("write did not commit"),
        _ = &mut pending => panic!("response arrived before the commit was observed"),
    }
    assert!(timeout(Duration::from_millis(30), pending).await.is_err());
    child.kill().await.unwrap();
    drop(client);
    let (client, _child) = connect(&db, "0").await;
    timeout(WAIT, async {
        let expected = json!({"ok":true,"code":"OK","revision":1,"value":21});
        assert_eq!(call(&client, "get_probe", json!({})).await, expected);
        assert_eq!(call(&client, "set_probe", args).await, expected);
        assert_eq!(
            call(
                &client,
                "set_probe",
                json!({"value":22,"idempotency_key":"next"})
            )
            .await["revision"],
            2
        );
        client.cancel().await.unwrap();
    })
    .await
    .unwrap();
}

async fn open(client: &RunningService<RoleClient, ()>, scenario: &str) -> Value {
    let result = call(
        client,
        "open_scenario",
        json!({"scenario_id":scenario,"version":1}),
    )
    .await;
    assert_eq!(result["ok"], true, "{result}");
    result["run_id"].clone()
}

#[tokio::test]
async fn p1_01_search_synonyms_filters_and_sorting() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("scenarios.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        let run = open(&client, "cnc").await;
        let find = |query: &str, effect: &str| {
            call(
                &client,
                "search_tools",
                json!({
                    "run_id":run,"query":query,"domain":"quality","effect":effect
                }),
            )
        };
        let chinese = find("质量冻结", "write").await;
        assert_eq!(chinese, find("Quality Hold", "write").await);
        assert_eq!(chinese["tools"].as_array().unwrap().len(), 1);
        assert_eq!(chinese["tools"][0]["name"], "hold_material");
        assert_eq!(find("质量冻结", "read").await["tools"], json!([]));
        let by_field = call(
            &client,
            "search_tools",
            json!({"run_id":run,"query":"idempotency_key"}),
        )
        .await;
        assert_eq!(by_field["tools"][0]["name"], "hold_material");
        let all = call(&client, "search_tools", json!({"run_id":run})).await;
        assert_eq!(
            all["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["get_material", "get_work_order", "hold_material"]
        );
        assert_eq!(
            all,
            call(&client, "search_tools", json!({"run_id":run})).await
        );
    })
    .await
    .unwrap();
}

async fn business(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    name: &str,
    arguments: Value,
) -> Value {
    call(
        client,
        "call_tool",
        json!({"run_id":run,"name":name,"arguments":arguments}),
    )
    .await
}

#[tokio::test]
async fn p1_02_schema_and_business_errors() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("scenarios.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        let cn = open(&client, "cnc").await;
        let sm = open(&client, "smt").await;
        let name = "hold_material";
        let contract = call(&client, "describe_tool", json!({"run_id":cn,"name":name})).await;
        assert_eq!(
            contract["tool"]["input"]["required"],
            json!(["batch", "reason", "idempotency_key"])
        );
        assert_eq!(contract["tool"]["effect"], "write");
        assert!(contract["tool"]["output"]["oneOf"].is_array());
        assert_eq!(
            business(&client, &cn, "invented_tool", json!({})).await["code"],
            "UNKNOWN_TOOL"
        );
        assert_eq!(
            business(&client, &sm, name, json!({})).await["code"],
            "TOOL_UNAVAILABLE"
        );
        assert_eq!(
            business(&client, &cn, name, json!({"batch":"LOT-1"})).await,
            json!({"ok":false,"code":"INVALID_ARGUMENT","field":"reason"})
        );
        assert_eq!(
            business(&client, &cn, "get_material", json!({"batch":"LOT-1"})).await["held"],
            false
        );
        let args = json!({"batch":"LOT-1","reason":"inspection","idempotency_key":"first"});
        let hold = business(&client, &cn, name, args.clone()).await;
        assert_eq!(hold["held"], true);
        assert_eq!(hold, business(&client, &cn, name, args).await);
        assert_eq!(
            business(
                &client,
                &cn,
                name,
                json!({"batch":"LOT-1","reason":"other","idempotency_key":"first"})
            )
            .await["code"],
            "IDEMPOTENCY_CONFLICT"
        );
        assert_eq!(
            business(&client, &cn, "get_material", json!({"batch":"LOT-1"})).await["cursor"],
            1
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p1_03_run_isolation_close_and_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("scenarios.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        let first = open(&client, "cnc").await;
        let second = open(&client, "smt").await;
        assert_ne!(first, second);
        let args = json!({"batch":"LOT-1","reason":"quarantine","idempotency_key":"test"});
        assert_eq!(
            business(&client, &first, "hold_material", args).await["cursor"],
            1
        );
        let (other, _other_child) = connect_mode(&db, "0", true).await;
        let batch = json!({"batch":"LOT-1"});
        assert_eq!(
            business(&other, &second, "get_material", batch.clone()).await["held"],
            false
        );
        assert_eq!(
            business(&other, &first, "get_material", batch.clone()).await["held"],
            true
        );
        let closed = call(&client, "close_run", json!({"run_id":first})).await;
        assert_eq!(closed["closed"], true);
        assert_eq!(closed["clock"], 0);
        assert_eq!(
            business(&other, &first, "get_material", batch).await["code"],
            "RUN_CLOSED"
        );
        let remaining = call(&other, "get_run", json!({"run_id":second})).await;
        assert_eq!(remaining["cursor"], 0);
        assert_eq!(remaining["clock"], 0);
        assert_eq!(remaining["closed"], false);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p1_04_register_and_reject_malformed_definitions() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("scenarios.sqlite3");
    let file = dir.path().join("definition.json");
    let mut definition: Value = serde_json::from_str(include_str!("../fixtures/cnc.json")).unwrap();
    definition["scenario_id"] = "external".into();
    definition["event_rules"] = json!([{"event":"supplier_arrival",
        "delay":{"distribution":"exponential","mean_minutes":12.5}}]);
    let register = |value: &Value| {
        std::fs::write(&file, value.to_string()).unwrap();
        std::process::Command::new(env!("CARGO_BIN_EXE_mom-mcp"))
            .arg("register-scenario")
            .arg("--db")
            .arg(&db)
            .arg("--file")
            .arg(&file)
            .output()
            .unwrap()
    };
    for (field, bad) in [
        ("materials", json!({"LOT-1":"NO-SUCH-ORDER"})),
        ("work_orders", json!("WO-1")),
        ("model_parameters", json!({"rule":{"occurred_at":17}})),
    ] {
        let mut malformed = definition.clone();
        malformed[field] = bad;
        let output = register(&malformed);
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("work order")
                || stderr.contains("sequence")
                || stderr.contains("timestamps"),
            "{stderr}"
        );
    }
    let (client, _child) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        let before = call(
            &client,
            "open_scenario",
            json!({"scenario_id":"external","version":1}),
        )
        .await;
        assert_eq!(before["code"], "SCENARIO_NOT_FOUND");
        assert!(register(&definition).status.success());
        assert!(
            !register(&definition).status.success(),
            "duplicate publication changed a version"
        );
        let run = open(&client, "external").await;
        assert_eq!(
            business(
                &client,
                &run,
                "get_work_order",
                json!({"work_order":"WO-1"})
            )
            .await["status"],
            "planned"
        );
    })
    .await
    .unwrap();
}

async fn advance(client: &RunningService<RoleClient, ()>, run: &Value, budget: u64) -> Value {
    call(
        client,
        "advance_run",
        json!({"run_id":run,"max_events":budget}),
    )
    .await
}

async fn delivered(client: &RunningService<RoleClient, ()>, run: &Value) -> Value {
    call(client, "read_events", json!({"run_id":run,"limit":100})).await
}

#[tokio::test]
async fn p2_01_shared_clock() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let (controller, _a) = connect_options(&db, "0", true, true).await;
    let (agent, _b) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        let a = open(&agent, "cnc").await;
        let b = open(&agent, "cnc").await;
        let args = json!({"run_id":a});
        let before = call(&agent, "get_run", args.clone()).await;
        assert_eq!(before, call(&agent, "get_run", args.clone()).await);
        assert_eq!(before["clock"], 0);
        let advanced = advance(&controller, &a, 1).await;
        assert!(advanced["clock"].as_u64().unwrap() > 0);
        assert_eq!(advanced["event_cursor"], 1);
        assert_eq!(
            call(&agent, "get_run", args.clone()).await["clock"],
            advanced["clock"]
        );
        assert_eq!(call(&controller, "get_run", args).await["event_cursor"], 1);
        assert_eq!(delivered(&agent, &b).await["clock"], 0);
    })
    .await
    .unwrap();
}

fn register_definition(db: &Path, definition: Value) -> std::process::Output {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("definition.json");
    std::fs::write(&file, definition.to_string()).unwrap();
    std::process::Command::new(env!("CARGO_BIN_EXE_mom-mcp"))
        .arg("register-scenario")
        .arg("--db")
        .arg(db)
        .arg("--file")
        .arg(file)
        .output()
        .unwrap()
}

#[tokio::test]
async fn p2_02_seeded_event_replay() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let fixture = json!({"scenario_id":"replay","version":1,"title":"Replay","model":"smt",
        "work_orders":["WO"],"materials":{},"capabilities":["get_work_order"],
        "event_rules":[{"event":"arrival","delay":{"distribution":"exponential","mean_minutes":21}},
            {"event":"processing","delay":{"distribution":"uniform","min_minutes":1,"max_minutes":10}}]});
    assert!(register_definition(&db, fixture).status.success());
    let (controller, _child) = connect_options(&db, "0", true, true).await;
    timeout(WAIT, async {
        let a = open(&controller, "replay").await;
        let b = open(&controller, "replay").await;
        let c = call(
            &controller,
            "open_scenario",
            json!({"scenario_id":"replay","version":1,"seed":17}),
        )
        .await["run_id"]
            .clone();
        for run in [&a, &b, &c] {
            assert_eq!(advance(&controller, run, 12).await["event_cursor"], 12);
        }
        let a = delivered(&controller, &a).await;
        assert_eq!(a, delivered(&controller, &b).await);
        assert_ne!(a, delivered(&controller, &c).await);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p2_03_budgets_and_cursor_paging() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let rules: Vec<_> = (0..17)
        .map(|index| {
            json!({"event":format!("event_{index}"),"delay":{
                "distribution":"uniform","min_minutes":1.1,"max_minutes":1.2
            }})
        })
        .collect();
    assert!(
        register_definition(
            &db,
            json!({"scenario_id":"crowded","version":1,"title":"Crowded",
        "model":"smt","work_orders":["WO"],"materials":{},"capabilities":[],"event_rules":rules})
        )
        .status
        .success()
    );
    let (controller, _child) = connect_options(&db, "0", true, true).await;
    timeout(WAIT, async {
        let run = open(&controller, "crowded").await;
        let first = advance(&controller, &run, 1).await;
        assert_eq!(first["stop"], "event_budget");
        let timed = call(
            &controller,
            "advance_run",
            json!({"run_id":run,"max_events":10,"max_minutes":1}),
        )
        .await;
        assert_eq!(timed["stop"], "time_budget");
        assert_eq!(timed["clock"], 1);
        let capped = advance(&controller, &run, 100).await;
        assert_eq!(capped["stop"], "same_tick_limit");
        assert_eq!(capped["event_cursor"], 18);
        let page_args = json!({"run_id":run,"after":2,"limit":2});
        let page = call(&controller, "read_events", page_args.clone()).await;
        assert_eq!(page, call(&controller, "read_events", page_args).await);
        assert_eq!(page["events"][0]["kind"], "event_0");
        assert_eq!(page["events"][1]["kind"], "event_1");
        assert_eq!(page["event_cursor"], 4);
        let rest = call(
            &controller,
            "read_events",
            json!({"run_id":run,"after":4,"limit":100}),
        )
        .await;
        assert_eq!(rest["events"][0]["cursor"], 5);
        assert_eq!(rest["has_more"], false);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p2_04_intervention_changes_future() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let (controller, _child) = connect_options(&db, "0", true, true).await;
    timeout(WAIT, async {
        let reference = open(&controller, "cnc").await;
        let intervention = open(&controller, "cnc").await;
        let baseline = advance(&controller, &reference, 1).await;
        let prior = delivered(&controller, &intervention).await;
        let control = call(
            &controller,
            "intervene_run",
            json!({"run_id":intervention,"action":"maintenance"}),
        )
        .await;
        assert_eq!(control["clock"], prior["clock"]);
        assert_eq!(delivered(&controller, &intervention).await, prior);
        advance(&controller, &intervention, 4).await;
        let changed = delivered(&controller, &intervention).await;
        assert_eq!(changed["events"][0]["kind"], "cnc_repair_started");
        assert_eq!(changed["events"][0]["minute"], prior["clock"]);
        assert_eq!(changed["events"][1]["kind"], "cnc_repair_completed");
        assert!(
            changed["events"][1]["minute"].as_u64().unwrap() > prior["clock"].as_u64().unwrap()
        );
        assert!(changed["events"][2]["payload"]["vibration_mm_s"].is_number());
        assert!(changed["events"][3]["payload"]["cpk"].is_number());
        let replay = call(&controller, "inspect_run", json!({"run_id":intervention})).await;
        assert_eq!(replay["replay_verified"], true);
        assert_eq!(changed["events"][2]["kind"], "cnc_vibration");
        assert_ne!(changed["events"][2]["minute"], baseline["clock"]);
        assert_eq!(
            delivered(&controller, &reference).await["events"][0]["kind"],
            "cnc_vibration"
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p2_05_model_registry_validation() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let mut spec: Value = serde_json::from_str(include_str!("../fixtures/cnc.json")).unwrap();
    spec["scenario_id"] = "model_test".into();
    spec["model_version"] = 7.into();
    let bad_version = register_definition(&db, spec.clone());
    assert!(!bad_version.status.success());
    assert!(String::from_utf8_lossy(&bad_version.stderr).contains("model_version"));
    spec["model_version"] = 1.into();
    spec["model_parameters"] = json!({"alarm_mean_minutes":0});
    let bad_parameter = register_definition(&db, spec);
    assert!(!bad_parameter.status.success());
    assert!(String::from_utf8_lossy(&bad_parameter.stderr).contains("alarm_mean_minutes"));
    let legacy = json!({"scenario_id":"legacy","version":1,"title":"Legacy","model":"unregistered",
        "work_orders":["WO"],"materials":{},"capabilities":[]});
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO scenario_definitions VALUES ('legacy',1,?1)",
            [legacy.to_string()],
        )
        .unwrap();
    let (controller, _child) = connect_options(&db, "0", true, true).await;
    timeout(WAIT, async {
        let rejected = call(
            &controller,
            "open_scenario",
            json!({"scenario_id":"legacy","version":1}),
        )
        .await;
        assert_eq!(rejected["code"], "SCENARIO_CONFIGURATION_INVALID");
        for (id, kind) in [("smt", "fixture_ready"), ("cnc", "cnc_vibration")] {
            let run = open(&controller, id).await;
            advance(&controller, &run, 1).await;
            assert_eq!(
                delivered(&controller, &run).await["events"][0]["kind"],
                kind
            );
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p2_06_control_visibility() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let (controller, _a) = connect_options(&db, "0", true, true).await;
    let (agent, _b) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        let run = open(&agent, "cnc").await;
        let args = json!({"run_id":run});
        let tools = agent.peer().list_tools(None).await.unwrap().tools;
        assert!(!tools.iter().any(|tool| matches!(
            tool.name.as_ref(),
            "advance_run" | "intervene_run" | "inspect_run"
        )));
        for hidden in ["inspect_run", "advance_run", "intervene_run"] {
            assert_eq!(
                call(&agent, hidden, args.clone()).await["code"],
                "UNKNOWN_TOOL"
            );
        }
        let evaluation = call(&controller, "inspect_run", args.clone()).await;
        assert_eq!(evaluation["diagnosis"]["root_cause"], "spindle_wear");
        assert!(evaluation["replay"]["timeline"]["pending"].is_array());
        let public = delivered(&agent, &run).await;
        assert_eq!(public["events"], json!([]));
        for secret in ["diagnosis", "root_cause", "pending", "spindle_wear"] {
            assert!(!public.to_string().contains(secret));
            assert!(
                !call(&agent, "get_run", args.clone())
                    .await
                    .to_string()
                    .contains(secret)
            );
        }
    })
    .await
    .unwrap();
}

async fn cnc(client: &RunningService<RoleClient, ()>) -> Value {
    let state = call(
        client,
        "open_scenario",
        json!({"scenario_id":"cnc","version":2}),
    )
    .await;
    assert_eq!(state["ok"], true);
    state["run_id"].clone()
}

async fn mes(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    tool: &str,
    order: &str,
    key: &str,
    extra: Value,
) -> Value {
    let mut args = extra.as_object().unwrap().clone();
    args.insert("work_order".into(), order.into());
    args.insert("idempotency_key".into(), key.into());
    business(client, run, tool, Value::Object(args)).await
}

async fn mes_status(client: &RunningService<RoleClient, ()>, run: &Value) -> Value {
    business(client, run, "mes_status", json!({"work_order":"WO-1"})).await
}

async fn ready_for_report(client: &RunningService<RoleClient, ()>, run: &Value, order: &str) {
    for (name, key, args) in [
        ("release_order", "release", json!({})),
        (
            "dispatch_operation",
            "dispatch",
            json!({"operation":"machining"}),
        ),
        ("issue_material", "issue", json!({"batch":"LOT-1"})),
        ("start_operation", "start", json!({"operation":"machining"})),
    ] {
        let response = mes(client, run, name, order, &format!("{order}-{key}"), args).await;
        assert_eq!(response["ok"], true, "{name}: {response}");
    }
}

async fn machining(client: &RunningService<RoleClient, ()>, run: &Value) -> Value {
    ready_for_report(client, run, "WO-1").await;
    let response = mes(
        client,
        run,
        "report_operation",
        "WO-1",
        "machining-report",
        json!({"operation":"machining"}),
    )
    .await;
    assert_eq!(response["ok"], true, "{response}");
    response
}

async fn inspect_cnc(client: &RunningService<RoleClient, ()>, run: &Value, result: &str) {
    machining(client, run).await;
    for (name, key, parameters) in [
        (
            "dispatch_operation",
            "dispatch-assembly",
            json!({"operation":"assembly"}),
        ),
        (
            "start_operation",
            "start-assembly",
            json!({"operation":"assembly"}),
        ),
        (
            "report_operation",
            "report-assembly",
            json!({"operation":"assembly"}),
        ),
        (
            "dispatch_operation",
            "dispatch-inspection",
            json!({"operation":"inspection"}),
        ),
        ("inspect_units", "inspect", json!({"result":result})),
    ] {
        let response = mes(client, run, name, "WO-1", key, parameters).await;
        assert_eq!(response["ok"], true, "{name}: {response}");
    }
}

#[tokio::test]
async fn p3_01_shortage_receipt_and_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_options(&dir.path().join("runs.sqlite3"), "0", true, true).await;
    timeout(WAIT,async {
        let run=open(&client,"cnc-shortage").await;
        let release=|key| mes(&client,&run,"release_order","WO-1",key,json!({}));
        let refusal=release("release").await;
        assert_eq!(refusal,json!({"ok":false,"code":"MATERIAL_SHORTAGE","batch":"LOT-1","missing":10,"location":"RAW-01"}));
        advance(&client,&run,1).await;
        assert_eq!(mes_status(&client,&run).await["on_hand"],10);
        assert_eq!(mes_status(&client,&run).await["cursor"],1);
        assert_eq!(mes_status(&client,&run).await["available_actions"],json!(["release_order"]));
        assert_eq!(release("release").await["state"]["route_version"],"route-1");
        assert_eq!(business(&client,&run,"revise_route",json!({"route_version":"route-2","idempotency_key":"revised"})).await["ok"],true);
        let status=mes_status(&client,&run).await;
        assert_eq!(status["route_master"],"route-2");
        assert_eq!(status["state"]["route_version"],"route-1");
    }).await.unwrap();
}

#[tokio::test]
async fn p3_02_sequence_and_normal_production() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        let run = cnc(&client).await;
        assert_eq!(
            mes(
                &client,
                &run,
                "start_operation",
                "WO-1",
                "skip",
                json!({"operation":"assembly"})
            )
            .await["code"],
            "OPERATION_OUT_OF_SEQUENCE"
        );
        assert_eq!(
            mes(
                &client,
                &run,
                "release_order",
                "WO-1",
                "stale",
                json!({"expected_cursor":99})
            )
            .await["code"],
            "VERSION_CONFLICT"
        );
        let equipment = |state, key| {
            business(
                &client,
                &run,
                "set_equipment",
                json!({"equipment_state":state,"idempotency_key":key}),
            )
        };
        assert_eq!(equipment("down", "down").await["equipment_down"], true);
        assert_eq!(
            mes(
                &client,
                &run,
                "release_order",
                "WO-2",
                "prepare-other",
                json!({})
            )
            .await["ok"],
            true
        );
        assert_eq!(
            mes(
                &client,
                &run,
                "dispatch_operation",
                "WO-2",
                "dispatch-other",
                json!({"operation":"machining"})
            )
            .await["ok"],
            true
        );
        assert_eq!(
            mes(
                &client,
                &run,
                "start_operation",
                "WO-2",
                "start-other",
                json!({"operation":"machining"})
            )
            .await["code"],
            "EQUIPMENT_UNAVAILABLE"
        );
        assert_eq!(
            equipment("available", "available").await["equipment_down"],
            false
        );
        inspect_cnc(&client, &run, "pass").await;
        assert_eq!(
            mes(&client, &run, "close_order", "WO-1", "finish", json!({})).await["state"]["phase"],
            "closed"
        );
        let status = mes_status(&client, &run).await;
        assert_eq!(status["on_hand"], 0);
        assert_eq!(status["state"]["units"].as_array().unwrap().len(), 10);
        assert_eq!(status["state"]["inspection"], "pass");
        assert_eq!(
            status["ledger"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["kind"] == "consumption")
                .unwrap()["delta"],
            -10
        );
        assert_eq!(
            business(
                &client,
                &run,
                "get_work_order",
                json!({"work_order":"WO-1"})
            )
            .await["status"],
            "closed"
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p3_03_idempotent_retry_and_parallel_stock() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    let (other, _other_child) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        let run = cnc(&client).await;
        for order in ["WO-1", "WO-2"] {
            ready_for_report(&client, &run, order).await;
        }
        let args = json!({"operation":"machining"});
        let (a, b) = tokio::join!(
            mes(
                &client,
                &run,
                "report_operation",
                "WO-1",
                "report-a",
                args.clone()
            ),
            mes(
                &other,
                &run,
                "report_operation",
                "WO-2",
                "report-b",
                args.clone()
            ),
        );
        let (winner, key, first, second) = if a["ok"] == true {
            ("WO-1", "report-a", a, b)
        } else {
            ("WO-2", "report-b", b, a)
        };
        assert_eq!(first["ok"], true);
        assert_eq!(second["code"], "MATERIAL_SHORTAGE");
        assert_eq!(
            mes(&other, &run, "report_operation", winner, key, args).await,
            first
        );
        assert_eq!(
            mes(
                &other,
                &run,
                "report_operation",
                winner,
                key,
                json!({"operation":"assembly"})
            )
            .await["code"],
            "IDEMPOTENCY_CONFLICT"
        );
        let status = mes_status(&client, &run).await;
        assert_eq!(status["on_hand"], 0);
        assert_eq!(status["ledger"].as_array().unwrap().len(), 1);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p3_04_rollback_and_receipt_deduplication() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let (client, _child) = connect_options(&db, "0", true, true).await;
    timeout(WAIT,async {
        let run=open(&client,"cnc-shortage").await;
        advance(&client,&run,3).await;
        advance(&client,&run,3).await;
        assert_eq!(mes_status(&client,&run).await["on_hand"],10);
        assert_eq!(mes_status(&client,&run).await["ledger"].as_array().unwrap().len(),1);
        ready_for_report(&client,&run,"WO-1").await;
        let sql=rusqlite::Connection::open(&db).unwrap();
        sql.execute_batch("CREATE TRIGGER fail_report BEFORE INSERT ON scenario_requests
            WHEN NEW.key='report-fault' BEGIN SELECT RAISE(ABORT,'test injection'); END;").unwrap();
        let arguments=json!({"run_id":run,"name":"report_operation","arguments":{"work_order":"WO-1","idempotency_key":"report-fault","operation":"machining"}});
        let result=client.peer().call_tool(CallToolRequestParams::new("call_tool").with_arguments(arguments.as_object().unwrap().clone())).await;
        assert!(result.is_err());
        let status=mes_status(&client,&run).await;
        assert_eq!(status["on_hand"],10);
        assert_eq!(status["reports"],json!([]));
        assert_eq!(business(&client,&run,"trace",json!({"subject":"LOT-1"})).await["links"],json!([]));
        sql.execute_batch("DROP TRIGGER fail_report").unwrap();
        let posted=mes(&client,&run,"report_operation","WO-1","report-fault",json!({"operation":"machining"})).await;
        assert_eq!(posted["ok"],true);
        assert_eq!(mes_status(&client,&run).await["on_hand"],0);
    }).await.unwrap();
}

#[tokio::test]
async fn p3_05_quality_disposition_and_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_options(&dir.path().join("runs.sqlite3"), "0", true, true).await;
    timeout(WAIT,async {
        for (run,cause,event,disposition) in [
            (cnc(&client).await,"spindle_wear","cnc_vibration","pass"),
            (open(&client,"cnc-lot").await,"incoming_material","incoming_lot_drift","scrap")
        ] {
            inspect_cnc(&client,&run,"fail").await;
            advance(&client,&run,1).await;
            let evidence=delivered(&client,&run).await;
            assert_eq!(evidence["events"][0]["kind"],event);
            assert_eq!(call(&client,"inspect_run",json!({"run_id":run})).await["diagnosis"]["root_cause"],cause);
            if cause == "spindle_wear" {
                assert_eq!(call(&client,"intervene_run",json!({"run_id":run,"action":"maintenance"})).await["ok"],true);
                advance(&client,&run,5).await;
            }
            assert_eq!(business(&client,&run,"hold_material",json!({"batch":"LOT-1","reason":"investigation","idempotency_key":"hold"})).await["held"],true);
            assert_eq!(mes(&client,&run,"close_order","WO-1","close",json!({})).await["code"],"QUALITY_HOLD");
            assert_eq!(mes(&client,&run,"disposition","WO-1","resolve",json!({"result":disposition,"reason":"reviewed evidence"})).await["state"]["inspection"],disposition);
            assert_eq!(mes(&client,&run,"close_order","WO-1","close",json!({})).await["ok"],true);
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p3_06_genealogy_and_report_reversal() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        let run = cnc(&client).await;
        let report = machining(&client, &run).await;
        let from_batch = business(&client, &run, "trace", json!({"subject":"LOT-1"})).await;
        assert_eq!(
            from_batch["links"][0]["serial_numbers"],
            report["serial_numbers"]
        );
        let from_unit = business(&client, &run, "trace", json!({"subject":"WO-1-01"})).await;
        assert_eq!(from_unit["links"][0]["batch"], "LOT-1");
        let reversed = mes(
            &client,
            &run,
            "reverse_report",
            "WO-1",
            "reverse",
            json!({"report_id":report["report"]["id"],"reason":"duplicate recording"}),
        )
        .await;
        assert_eq!(reversed["ok"], true);
        let status = mes_status(&client, &run).await;
        assert_eq!(status["on_hand"], 10);
        assert_eq!(status["reports"][0]["reversed"], true);
        assert_eq!(status["ledger"][1]["report_id"], report["report"]["id"]);
        assert_eq!(
            business(&client, &run, "trace", json!({"subject":"WO-1-01"})).await["links"],
            json!([])
        );
    })
    .await
    .unwrap();
}

async fn smt(client: &RunningService<RoleClient, ()>) -> Value {
    let opened = call(
        client,
        "open_scenario",
        json!({"scenario_id":"smt","version":2}),
    )
    .await;
    assert_eq!(opened["ok"], true, "{opened}");
    opened["run_id"].clone()
}

async fn smt_action(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    name: &str,
    key: &str,
    args: Value,
) -> Value {
    let mut args = args.as_object().unwrap().clone();
    args.insert("idempotency_key".into(), key.into());
    business(client, run, name, Value::Object(args)).await
}

async fn smt_panel(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    panel: &str,
    reel: &str,
) -> Value {
    let mut result = Value::Null;
    for (name, suffix, args) in [
        ("smt_load", "load", json!({"slot":"F1","reel":reel})),
        ("smt_place", "place", json!({"panel":panel,"slot":"F1"})),
        ("smt_aoi", "aoi", json!({"panel":panel,"result":"pass"})),
        ("smt_split", "split", json!({"panel":panel})),
    ] {
        result = smt_action(client, run, name, &format!("{panel}-{suffix}"), args).await;
        assert_eq!(result["ok"], true, "{name}: {result}");
    }
    result
}

async fn smt_population(client: &RunningService<RoleClient, ()>, run: &Value) {
    for order in ["WO-A", "WO-B"] {
        assert_eq!(
            smt_action(
                client,
                run,
                "smt_release",
                order,
                json!({"work_order":order})
            )
            .await["ok"],
            true
        );
    }
    for (panel, reel) in [("P-A", "REEL-R"), ("P-B", "REEL-R"), ("P-C", "REEL-C")] {
        smt_panel(client, run, panel, reel).await;
    }
}

async fn smt_recall(client: &RunningService<RoleClient, ()>, run: &Value) -> Value {
    advance(client, run, 10).await;
    let observed = delivered(client, run).await;
    observed["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == "supplier_recall")
        .unwrap()
        .clone()
}

#[tokio::test]
async fn p4_01_smt_normal_route_and_genealogy() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT,async {
        let run=smt(&client).await;
        assert_eq!(smt_action(&client,&run,"smt_place","early",json!({"panel":"P-A","slot":"F1"})).await["code"],"INVALID_STATE");
        assert_eq!(smt_action(&client,&run,"smt_release","release",json!({"work_order":"WO-A"})).await["ok"],true);
        assert_eq!(smt_action(&client,&run,"smt_load","load",json!({"slot":"F2","reel":"REEL-R"})).await["ok"],true);
        assert_eq!(smt_action(&client,&run,"smt_place","place",json!({"panel":"P-A","slot":"F2"})).await["ok"],true);
        assert_eq!(smt_action(&client,&run,"smt_split","split",json!({"panel":"P-A"})).await["code"],"AOI_REQUIRED");
        assert_eq!(smt_action(&client,&run,"smt_aoi","aoi",json!({"panel":"P-A","result":"pass"})).await["ok"],true);
        assert_eq!(smt_action(&client,&run,"smt_split","split",json!({"panel":"P-A"})).await["serial_numbers"],json!(["A-01","A-02"]));
        for serial in ["A-01","A-02"] {
            assert_eq!(smt_action(&client,&run,"smt_finish",serial,json!({"serial":serial})).await["finished"],true);
        }
        let board=business(&client,&run,"smt_trace",json!({"subject":"A-01"})).await;
        assert_eq!(board["links"][0]["panel"],"P-A");
        assert_eq!(board["links"][0]["reel"],"REEL-R");
        assert_eq!(board["links"][0]["machine"],"SMT-1");
        assert_eq!(board["links"][0]["slot"],"F2");
        assert_eq!(board["links"][0]["load_id"],1);
        assert_eq!(business(&client,&run,"smt_trace",json!({"subject":"P-A"})).await["links"].as_array().unwrap().len(),2);
        assert_eq!(business(&client,&run,"get_work_order",json!({"work_order":"WO-A"})).await["status"],"complete");
    }).await.unwrap();
}

#[tokio::test]
async fn p4_02_smt_recall_across_work_orders() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_options(&dir.path().join("runs.sqlite3"), "0", true, true).await;
    timeout(WAIT, async {
        let run = smt(&client).await;
        smt_population(&client, &run).await;
        assert_eq!(
            smt_action(
                &client,
                &run,
                "smt_isolate",
                "pending",
                json!({"event_cursor":1})
            )
            .await["code"],
            "RECALL_NOT_DELIVERED"
        );
        let status = business(&client, &run, "smt_status", json!({"work_order":"WO-B"})).await;
        assert_eq!(status["load_history"][2]["reel"], "REEL-C");
        assert_eq!(status["remaining"]["REEL-R"], 8);
        let event = smt_recall(&client, &run).await;
        assert!(event["minute"].as_u64().unwrap() > 0);
        let args = json!({"event_cursor":event["cursor"]});
        let isolated = smt_action(&client, &run, "smt_isolate", "recall", args.clone()).await;
        assert_eq!(
            isolated["affected"],
            json!(["A-01", "A-02", "B-01", "B-02"])
        );
        assert_eq!(isolated["affected_panels"], json!(["P-A", "P-B"]));
        assert_eq!(
            smt_action(&client, &run, "smt_isolate", "recall", args).await,
            isolated
        );
        let by_reel = business(&client, &run, "smt_trace", json!({"subject":"REEL-R"})).await;
        assert_eq!(by_reel["links"][0]["work_order"], "WO-A");
        assert_eq!(by_reel["links"][3]["work_order"], "WO-B");
        for (serial, expected) in [
            ("A-01", "QUALITY_HOLD"),
            ("B-02", "QUALITY_HOLD"),
            ("C-01", "OK"),
        ] {
            let result = smt_action(
                &client,
                &run,
                "smt_finish",
                serial,
                json!({"serial":serial}),
            )
            .await;
            if expected == "OK" {
                assert_eq!(result["finished"], true);
            } else {
                assert_eq!(result["code"], expected);
            }
        }
        assert_eq!(
            smt_action(
                &client,
                &run,
                "smt_load",
                "reload",
                json!({"slot":"F2","reel":"REEL-R"})
            )
            .await["code"],
            "REEL_RECALLED"
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p4_03_smt_rework_retest_scrap_and_replay() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_options(&dir.path().join("runs.sqlite3"), "0", true, true).await;
    // This workflow drives two runs with sequential RPCs. Each request still has a five-second deadline.
    timeout(Duration::from_secs(30), async {
        let run = smt(&client).await;
        smt_population(&client, &run).await;
        let event = smt_recall(&client, &run).await;
        let comparison = smt(&client).await;
        for (tool, args) in [
            ("smt_release", json!({"work_order":"WO-B"})),
            ("smt_load", json!({"slot":"F1","reel":"REEL-C"})),
            ("smt_place", json!({"panel":"P-B","slot":"F1"})),
        ] {
            assert_eq!(
                smt_action(&client, &comparison, tool, tool, args).await["ok"],
                true
            );
        }
        assert_eq!(smt_recall(&client, &comparison).await, event);
        assert_eq!(
            delivered(&client, &run).await,
            delivered(&client, &comparison).await
        );
        assert_eq!(
            smt_action(
                &client,
                &run,
                "smt_isolate",
                "isolate",
                json!({"event_cursor":event["cursor"]})
            )
            .await["ok"],
            true
        );
        assert_eq!(
            smt_action(
                &client,
                &comparison,
                "smt_aoi",
                "drift",
                json!({"panel":"P-B","result":"pass"})
            )
            .await["code"],
            "AOI_DRIFT_DETECTED"
        );
        assert_eq!(
            smt_action(
                &client,
                &comparison,
                "smt_aoi",
                "drift",
                json!({"panel":"P-B","result":"fail"})
            )
            .await["ok"],
            true
        );
        let rework = |key| {
            smt_action(
                &client,
                &run,
                "smt_rework",
                key,
                json!({"serial":"A-01","replacement_reel":"REEL-C","reason":"supplier recall"}),
            )
        };
        assert_eq!(rework("rework").await["quality"], "awaiting_retest");
        assert_eq!(
            smt_action(
                &client,
                &run,
                "smt_finish",
                "early",
                json!({"serial":"A-01"})
            )
            .await["code"],
            "RETEST_REQUIRED"
        );
        assert_eq!(
            smt_action(
                &client,
                &run,
                "smt_retest",
                "fail",
                json!({"serial":"A-01","result":"fail"})
            )
            .await["quality"],
            "held"
        );
        assert_eq!(rework("second-rework").await["quality"], "awaiting_retest");
        assert_eq!(
            smt_action(
                &client,
                &run,
                "smt_retest",
                "pass",
                json!({"serial":"A-01","result":"pass"})
            )
            .await["quality"],
            "clear"
        );
        assert_eq!(
            smt_action(
                &client,
                &run,
                "smt_finish",
                "finish",
                json!({"serial":"A-01"})
            )
            .await["finished"],
            true
        );
        assert_eq!(
            smt_action(
                &client,
                &run,
                "smt_scrap",
                "scrap",
                json!({"serial":"A-02","reason":"damaged board"})
            )
            .await["quality"],
            "scrapped"
        );
        assert_eq!(
            smt_action(
                &client,
                &run,
                "smt_finish",
                "reject-scrap",
                json!({"serial":"A-02"})
            )
            .await["code"],
            "BOARD_SCRAPPED"
        );
        let trace = business(&client, &run, "smt_trace", json!({"subject":"A-01"})).await;
        assert_eq!(trace["links"][0]["reel"], "REEL-R");
        assert_eq!(
            trace["links"][0]["replacements"],
            json!(["REEL-C", "REEL-C"])
        );
        assert!(
            trace["links"][0]["history"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["action"] == "retest" && item["result"] == "pass")
        );
    })
    .await
    .expect("SMT rework and replay workflow exceeded its 30-second budget");
}

async fn open_assembly(client: &RunningService<RoleClient, ()>, normal: bool) -> Value {
    let (id, version) = if normal {
        ("assembly", 2)
    } else {
        ("assembly-shortage", 1)
    };
    let value = call(
        client,
        "open_scenario",
        json!({"scenario_id":id,"version":version}),
    )
    .await;
    assert_eq!(value["ok"], true, "{value}");
    value["run_id"].clone()
}

async fn assembly_action(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    action: &str,
    order: &str,
    key: &str,
    args: Value,
) -> Value {
    let mut payload = args.as_object().unwrap().clone();
    payload.insert("work_order".into(), order.into());
    payload.insert("idempotency_key".into(), key.into());
    business(client, run, action, Value::Object(payload)).await
}

async fn build_assembly(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    order: &str,
    cell: &str,
) {
    for (tool, args) in [
        ("assembly_dispatch", json!({"cell":cell})),
        ("assembly_build", json!({})),
    ] {
        let result =
            assembly_action(client, run, tool, order, &format!("{order}-{tool}"), args).await;
        assert_eq!(result["ok"], true, "{tool}: {result}");
    }
}

async fn assembly_kit(client: &RunningService<RoleClient, ()>, run: &Value, order: &str) -> Value {
    business(client, run, "assembly_kit", json!({"work_order":order})).await
}

#[tokio::test]
async fn p5_01_assembly_kit_build_test_and_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_options(&dir.path().join("runs.sqlite3"), "0", true, true).await;
    timeout(WAIT, async {
        let run = open_assembly(&client, true).await;
        let kit = assembly_kit(&client, &run, "WO-E").await;
        assert_eq!(kit["ready"], true);
        assert_eq!(kit["bom"], json!({"DRIVE-STD":1,"FRAME":1}));
        assert_eq!(kit["due_in_minutes"], 35);
        assert_eq!(
            assembly_action(
                &client,
                &run,
                "assembly_dispatch",
                "WO-E",
                "wrong-cell",
                json!({"cell":"CELL-B"})
            )
            .await["code"],
            "CELL_NOT_CAPABLE"
        );
        build_assembly(&client, &run, "WO-E", "CELL-A").await;
        assert_eq!(
            assembly_kit(&client, &run, "WO-E").await["stock"]["FRAME"],
            1
        );
        let started = assembly_action(
            &client,
            &run,
            "assembly_start_test",
            "WO-E",
            "start",
            json!({}),
        )
        .await;
        assert_eq!(started["occupied_by"], "WO-E");
        assert_eq!(
            assembly_action(
                &client,
                &run,
                "assembly_finish_test",
                "WO-E",
                "finish",
                json!({})
            )
            .await["code"],
            "TEST_NOT_READY"
        );
        advance(&client, &run, 10).await;
        let passed = assembly_action(
            &client,
            &run,
            "assembly_finish_test",
            "WO-E",
            "finish",
            json!({}),
        )
        .await;
        assert_eq!(passed["state"]["tests"][0]["result"], "pass");
        assert_eq!(passed["occupied_by"], Value::Null);
        let delivery = assembly_action(
            &client,
            &run,
            "assembly_deliver",
            "WO-E",
            "deliver",
            json!({}),
        )
        .await;
        assert_eq!(delivery["state"]["delivery"]["serial"], "UNIT-E-01");
        let trace = business(
            &client,
            &run,
            "assembly_trace",
            json!({"subject":"UNIT-E-01"}),
        )
        .await;
        assert_eq!(trace["links"][0]["installed"], kit["bom"]);
        assert_eq!(trace["links"][0]["work_cell"], "CELL-A");
        assert_eq!(trace["links"][0]["delivery"]["status"], "delivered");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p5_02_shortage_qualified_substitution_and_priority() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_options(&dir.path().join("runs.sqlite3"), "0", true, true).await;
    timeout(WAIT, async {
        let run = open_assembly(&client, false).await;
        let initial = assembly_kit(&client, &run, "WO-E").await;
        assert_eq!(initial["ready"], false);
        assert_eq!(initial["missing"], json!({"DRIVE-STD":1}));
        assert_eq!(
            assembly_action(
                &client,
                &run,
                "assembly_substitute",
                "WO-E",
                "unqualified",
                json!({"part":"DRIVE-BAD"})
            )
            .await["code"],
            "SUBSTITUTE_NOT_QUALIFIED"
        );
        assert_eq!(initial, assembly_kit(&client, &run, "WO-E").await);
        assert_eq!(
            assembly_action(
                &client,
                &run,
                "assembly_dispatch",
                "WO-E",
                "missing-stock",
                json!({"cell":"CELL-A"})
            )
            .await["code"],
            "MATERIAL_SHORTAGE"
        );
        assert_eq!(
            assembly_action(
                &client,
                &run,
                "assembly_substitute",
                "WO-E",
                "qualified",
                json!({"part":"DRIVE-ALT"})
            )
            .await["qualification"],
            "QA-DRIVE-017"
        );
        build_assembly(&client, &run, "WO-E", "CELL-A").await;
        advance(&client, &run, 10).await;
        advance(&client, &run, 10).await;
        assert_eq!(
            assembly_kit(&client, &run, "WO-L").await["stock"]["DRIVE-STD"],
            1
        );
        build_assembly(&client, &run, "WO-L", "CELL-B").await;
        let trace = business(
            &client,
            &run,
            "assembly_trace",
            json!({"subject":"DRIVE-ALT"}),
        )
        .await;
        assert_eq!(trace["links"][0]["installed"]["DRIVE-ALT"], 1);
        assert_eq!(
            trace["links"][0]["substitution_qualification"],
            "QA-DRIVE-017"
        );
        assert_eq!(trace["queue"], json!(["WO-E", "WO-L"]));
        let priority = assembly_action(
            &client,
            &run,
            "assembly_priority",
            "WO-L",
            "urgent",
            json!({"priority":5}),
        )
        .await;
        assert_eq!(priority["queue"], json!(["WO-L", "WO-E"]));
        assert_eq!(
            assembly_action(
                &client,
                &run,
                "assembly_start_test",
                "WO-E",
                "queue",
                json!({})
            )
            .await["code"],
            "TEST_PRIORITY_BLOCKED"
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p5_03_shared_bench_failure_repair_and_retest() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let (client, _a) = connect_options(&db, "0", true, true).await;
    let (other, _b) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        let run = open_assembly(&client, false).await;
        assert_eq!(
            assembly_action(
                &client,
                &run,
                "assembly_substitute",
                "WO-E",
                "sub",
                json!({"part":"DRIVE-ALT"})
            )
            .await["ok"],
            true
        );
        build_assembly(&client, &run, "WO-E", "CELL-A").await;
        advance(&client, &run, 10).await;
        build_assembly(&client, &run, "WO-L", "CELL-B").await;
        let first = assembly_action(
            &client,
            &run,
            "assembly_start_test",
            "WO-E",
            "start-e",
            json!({}),
        )
        .await;
        assert_eq!(first["occupied_by"], "WO-E");
        assert_eq!(
            assembly_action(
                &other,
                &run,
                "assembly_start_test",
                "WO-E",
                "start-e",
                json!({})
            )
            .await,
            first
        );
        assert_eq!(
            assembly_action(
                &other,
                &run,
                "assembly_start_test",
                "WO-L",
                "start-l",
                json!({})
            )
            .await["code"],
            "TEST_BENCH_BUSY"
        );
        advance(&client, &run, 10).await;
        let failed = assembly_action(
            &other,
            &run,
            "assembly_finish_test",
            "WO-E",
            "finish-e",
            json!({}),
        )
        .await;
        assert_eq!(failed["state"]["phase"], "failed");
        assert_eq!(
            assembly_action(
                &client,
                &run,
                "assembly_deliver",
                "WO-E",
                "deliver",
                json!({})
            )
            .await["code"],
            "TEST_PASS_REQUIRED"
        );
        assert_eq!(
            assembly_action(
                &client,
                &run,
                "assembly_repair",
                "WO-E",
                "repair",
                json!({"reason":"re-seat drive connector"})
            )
            .await["state"]["phase"],
            "queued"
        );
        assert_eq!(
            assembly_action(
                &other,
                &run,
                "assembly_start_test",
                "WO-E",
                "retry",
                json!({})
            )
            .await["ok"],
            true
        );
        advance(&client, &run, 10).await;
        let passed = assembly_action(
            &other,
            &run,
            "assembly_finish_test",
            "WO-E",
            "retest",
            json!({}),
        )
        .await;
        let tests = &passed["state"]["tests"];
        assert_eq!(tests[1]["result"], "pass");
        assert!(
            tests[1]["start_minute"].as_u64().unwrap() >= tests[0]["end_minute"].as_u64().unwrap()
        );
        assert_eq!(
            assembly_action(
                &other,
                &run,
                "assembly_deliver",
                "WO-E",
                "deliver",
                json!({})
            )
            .await["state"]["phase"],
            "delivered"
        );
        assert_eq!(
            assembly_action(
                &other,
                &run,
                "assembly_start_test",
                "WO-L",
                "start-l",
                json!({})
            )
            .await["occupied_by"],
            "WO-L"
        );
        let replay = call(&client, "inspect_run", json!({"run_id":run})).await;
        assert_eq!(replay["replay_verified"], true);
        assert_eq!(
            replay["replay"]["timeline"]["events"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|event| event["kind"] == "assembly_test_complete")
                .count(),
            2
        );
    })
    .await
    .unwrap();
}

fn write_acceptance_evidence(name: &str, evidence: Value) {
    if let Some(directory) = std::env::var_os("MOM_ACCEPTANCE_RESULTS") {
        let path = std::path::PathBuf::from(directory).join(name);
        std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    }
}

#[tokio::test]
async fn p6_01_cross_industry_catalog_and_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        let cnc = cnc(&client).await;
        let smt = smt(&client).await;
        let assembly = open_assembly(&client, true).await;
        for (run, own, foreign) in [
            (&cnc, "mes_status", "smt_status"),
            (&smt, "smt_status", "assembly_kit"),
            (&assembly, "assembly_kit", "mes_status"),
        ] {
            let found = call(&client, "search_tools", json!({"run_id":run})).await;
            let tools = found["tools"].as_array().unwrap();
            assert!(tools.iter().any(|tool| tool["name"] == own));
            assert!(!tools.iter().any(|tool| tool["name"] == foreign));
            assert_eq!(
                business(&client, run, foreign, json!({})).await["code"],
                "TOOL_UNAVAILABLE"
            );
        }
        assert_eq!(
            business(
                &client,
                &cnc,
                "hold_material",
                json!({"batch":"LOT-1","reason":"test","idempotency_key":"cross"})
            )
            .await["held"],
            true
        );
        assert_eq!(
            smt_action(
                &client,
                &smt,
                "smt_release",
                "cross",
                json!({"work_order":"WO-A"})
            )
            .await["ok"],
            true
        );
        assert_eq!(
            assembly_action(
                &client,
                &assembly,
                "assembly_priority",
                "WO-E",
                "cross",
                json!({"priority":4})
            )
            .await["ok"],
            true
        );
        for run in [&cnc, &smt, &assembly] {
            let view = call(&client, "get_run", json!({"run_id":run})).await;
            assert_eq!(view["cursor"], 1);
            assert_eq!(view["clock"], 0);
            assert_eq!(
                call(&client, "close_run", json!({"run_id":run})).await["closed"],
                true
            );
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p6_02_multi_seed_business_invariants() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_options(&dir.path().join("runs.sqlite3"), "0", true, true).await;
    timeout(WAIT,async {
        let mut report=Vec::new();
        for (scenario,version) in [("cnc",2),("smt",2),("assembly-shortage",1)] {
            let mut traces=Vec::new();
            for seed in [42,42,17] {
                let opened=call(&client,"open_scenario",json!({"scenario_id":scenario,"version":version,"seed":seed})).await;
                assert_eq!(opened["ok"],true);
                let run=&opened["run_id"];
                let evidence=match scenario {
                    "cnc" => {
                        inspect_cnc(&client,run,"fail").await;
                        advance(&client,run,10).await;
                        assert_eq!(mes(&client,run,"close_order","WO-1","blocked",json!({})).await["code"],"QUALITY_HOLD");
                        assert_eq!(mes(&client,run,"disposition","WO-1","retest",json!({"result":"pass","reason":"reviewed"})).await["ok"],true);
                        assert_eq!(mes(&client,run,"close_order","WO-1","close",json!({})).await["ok"],true);
                        let status=mes_status(&client,run).await;
                        assert_eq!(status["on_hand"],0);
                        status["state"].clone()
                    }
                    "smt" => {
                        smt_population(&client,run).await;
                        let event=smt_recall(&client,run).await;
                        let isolation=smt_action(&client,run,"smt_isolate","isolate",json!({"event_cursor":event["cursor"]})).await;
                        assert_eq!(isolation["affected"],json!(["A-01","A-02","B-01","B-02"]));
                        assert_eq!(smt_action(&client,run,"smt_scrap","scrap",json!({"serial":"A-01","reason":"hold"})).await["ok"],true);
                        assert_eq!(smt_action(&client,run,"smt_finish","clean",json!({"serial":"C-01"})).await["finished"],true);
                        isolation["affected"].clone()
                    }
                    _ => {
                        assert_eq!(assembly_action(&client,run,"assembly_substitute","WO-E","sub",json!({"part":"DRIVE-ALT"})).await["ok"],true);
                        build_assembly(&client,run,"WO-E","CELL-A").await;
                        for (prefix,expected) in [("first","fail"),("retry","pass")] {
                            assert_eq!(assembly_action(&client,run,"assembly_start_test","WO-E",&format!("{prefix}-start"),json!({})).await["ok"],true);
                            advance(&client,run,10).await;
                            let finish=assembly_action(&client,run,"assembly_finish_test","WO-E",&format!("{prefix}-finish"),json!({})).await;
                            let attempts=finish["state"]["tests"].as_array().unwrap();
                            assert_eq!(attempts.last().unwrap()["result"],expected);
                            if expected=="fail" {
                                assert_eq!(assembly_action(&client,run,"assembly_repair","WO-E","repair",json!({"reason":"re-seat connector"})).await["ok"],true);
                            }
                        }
                        let delivered=assembly_action(&client,run,"assembly_deliver","WO-E","deliver",json!({})).await;
                        assert_eq!(delivered["state"]["phase"],"delivered");
                        delivered["state"]["installed"].clone()
                    }
                };
                let events=delivered(&client,run).await;
                let inspection=call(&client,"inspect_run",json!({"run_id":run})).await;
                assert_eq!(inspection["replay_verified"],true);
                assert!(events["events"].as_array().unwrap().iter().all(|e| e["payload"].get("root_cause").is_none()));
                assert_eq!(call(&client,"close_run",json!({"run_id":run})).await["closed"],true);
                traces.push(json!({"seed":seed,"events":events,"business":evidence}));
            }
            assert_eq!(traces[0],traces[1]);
            assert_ne!(traces[0]["events"],traces[2]["events"],"{scenario}");
            assert_eq!(traces[0]["business"],traces[2]["business"]);
            report.push(json!({"scenario":scenario,"version":version,"variants":traces}));
        }
        write_acceptance_evidence("industry-replay.json",json!(report));
    }).await.unwrap();
}

#[tokio::test]
async fn p6_03_restart_budget_and_run_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let (controller, mut child) = connect_options(&db, "0", true, true).await;
    timeout(WAIT, async {
        let run = open_assembly(&controller, true).await;
        let independent = cnc(&controller).await;
        build_assembly(&controller, &run, "WO-E", "CELL-A").await;
        let test = assembly_action(
            &controller,
            &run,
            "assembly_start_test",
            "WO-E",
            "durable",
            json!({}),
        )
        .await;
        assert_eq!(test["state"]["phase"], "testing");
        assert_eq!(
            call(
                &controller,
                "advance_run",
                json!({"run_id":run,"max_events":101})
            )
            .await,
            json!({"ok":false,"code":"INVALID_ARGUMENT","field":"max_events"})
        );
        child.kill().await.unwrap();
        drop(controller);

        let (agent, _a) = connect_mode(&db, "0", true).await;
        let (restored, _b) = connect_options(&db, "0", true, true).await;
        assert_eq!(
            call(&agent, "advance_run", json!({"run_id":run})).await["code"],
            "UNKNOWN_TOOL"
        );
        assert_eq!(
            assembly_action(
                &agent,
                &run,
                "assembly_start_test",
                "WO-E",
                "durable",
                json!({})
            )
            .await,
            test
        );
        let (step, read) = tokio::join!(
            advance(&restored, &run, 1),
            call(&agent, "get_run", json!({"run_id":run}))
        );
        assert_eq!(read["ok"], true);
        assert_eq!(step["event_cursor"], 1);
        let events = delivered(&agent, &run).await;
        assert_eq!(events["events"].as_array().unwrap().len(), 1);
        assert_eq!(
            assembly_action(
                &agent,
                &run,
                "assembly_finish_test",
                "WO-E",
                "finish",
                json!({})
            )
            .await["state"]["tests"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            call(&agent, "close_run", json!({"run_id":run})).await["closed"],
            true
        );
        let still_open = call(&agent, "get_run", json!({"run_id":independent})).await;
        assert_eq!(still_open["closed"], false);
        assert_eq!(still_open["event_cursor"], 0);
        assert_eq!(
            call(&agent, "close_run", json!({"run_id":independent})).await["closed"],
            true
        );
        write_acceptance_evidence(
            "restart-recovery.json",
            json!({"before_restart":test,"after_restart":events,"independent":still_open}),
        );
    })
    .await
    .unwrap();
}

async fn shop_action(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    name: &str,
    key: &str,
    arguments: Value,
) -> Value {
    let mut arguments = arguments.as_object().unwrap().clone();
    arguments.insert("idempotency_key".into(), key.into());
    business(client, run, name, Value::Object(arguments)).await
}

async fn finish_sheet(client: &RunningService<RoleClient, ()>, run: &Value) {
    for (name, args) in [
        ("sheet_form", json!({})),
        ("sheet_inspect", json!({"result":"pass"})),
        ("sheet_return", json!({})),
    ] {
        let result = shop_action(client, run, name, name, args).await;
        assert_eq!(result["ok"], true, "{name}: {result}");
    }
}

#[tokio::test]
async fn p7_01_sheet_area_conservation_and_remnant_lineage() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_options(&dir.path().join("runs.sqlite3"), "0", true, true).await;
    timeout(WAIT, async {
        let run = open(&client, "sheet-metal").await;
        advance(&client, &run, 10).await;
        assert_eq!(
            shop_action(
                &client,
                &run,
                "sheet_issue",
                "issue",
                json!({"stock":"SHEET-FULL"})
            )
            .await["code"],
            "STOCK_UNAVAILABLE"
        );
        assert_eq!(
            shop_action(
                &client,
                &run,
                "sheet_issue",
                "issue",
                json!({"stock":"REM-OK"})
            )
            .await["ok"],
            true
        );
        assert_eq!(
            shop_action(&client, &run, "sheet_nest", "nest", json!({})).await["ok"],
            true
        );
        assert_eq!(
            shop_action(
                &client,
                &run,
                "sheet_cut",
                "cut",
                json!({"machine":"LASER-B"})
            )
            .await["ok"],
            true
        );
        finish_sheet(&client, &run).await;
        let history = business(&client, &run, "sheet_trace", json!({"subject":"LOT-AL-2"})).await;
        let cut = &history["ledger"][0];
        let number = |field: &str| cut[field].as_u64().unwrap();
        assert_eq!(
            number("input_mm2"),
            number("product_mm2") + number("scrap_mm2") + number("remnant_mm2")
        );
        assert_eq!(history["area_unit"], "mm2");
        assert_eq!(history["phase"], "complete");
        let returned = &history["stock"]["REM-RETURN"];
        assert_eq!(returned["area_mm2"], 300);
        assert_eq!(returned["parent"], "REM-OK");
        assert_eq!(history["stock"]["REM-OK"]["parent"], "SHEET-AL-2");
        assert_eq!(returned["lot"], "LOT-AL-2");
        let normal = open(&client, "sheet-metal").await;
        for (name, args) in [
            ("sheet_issue", json!({"stock":"SHEET-FULL"})),
            ("sheet_nest", json!({})),
            ("sheet_cut", json!({"machine":"LASER-A"})),
        ] {
            assert_eq!(
                shop_action(&client, &normal, name, name, args).await["ok"],
                true
            );
        }
        finish_sheet(&client, &normal).await;
        let original = business(
            &client,
            &normal,
            "sheet_trace",
            json!({"subject":"LOT-AL-1"}),
        )
        .await;
        assert_eq!(original["stock"]["REM-RETURN"]["area_mm2"], 1100);
        for completed in [&run, &normal] {
            assert_eq!(
                call(&client, "close_run", json!({"run_id":completed})).await["closed"],
                true
            );
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p7_02_invalid_stock_and_sheet_machine_reroute() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_options(&dir.path().join("runs.sqlite3"), "0", true, true).await;
    timeout(WAIT, async {
        let run = open(&client, "sheet-metal").await;
        let before = business(&client, &run, "sheet_trace", json!({"subject":"WO-SHEET"})).await;
        for (stock, code) in [
            ("REM-BAD", "STOCK_INELIGIBLE"),
            ("REM-SMALL", "INSUFFICIENT_AREA"),
        ] {
            assert_eq!(
                shop_action(
                    &client,
                    &run,
                    "sheet_issue",
                    "issue",
                    json!({"stock":stock})
                )
                .await["code"],
                code
            );
        }
        assert_eq!(
            business(&client, &run, "sheet_trace", json!({"subject":"WO-SHEET"})).await,
            before
        );
        advance(&client, &run, 10).await;
        assert_eq!(
            shop_action(
                &client,
                &run,
                "sheet_issue",
                "issue",
                json!({"stock":"REM-OK"})
            )
            .await["ok"],
            true
        );
        assert_eq!(
            shop_action(&client, &run, "sheet_nest", "nest", json!({})).await["ok"],
            true
        );
        let before_cut =
            business(&client, &run, "sheet_trace", json!({"subject":"WO-SHEET"})).await;
        for (machine, code) in [
            ("PRESS-1", "MACHINE_NOT_CAPABLE"),
            ("LASER-A", "MACHINE_UNAVAILABLE"),
        ] {
            assert_eq!(
                shop_action(
                    &client,
                    &run,
                    "sheet_cut",
                    "cut",
                    json!({"machine":machine})
                )
                .await["code"],
                code
            );
        }
        assert_eq!(
            business(&client, &run, "sheet_trace", json!({"subject":"WO-SHEET"})).await,
            before_cut
        );
        let cut = shop_action(
            &client,
            &run,
            "sheet_cut",
            "cut",
            json!({"machine":"LASER-B"}),
        )
        .await;
        assert_eq!(cut["phase"], "cut");
        assert_eq!(
            shop_action(
                &client,
                &run,
                "sheet_cut",
                "cut",
                json!({"machine":"LASER-B"})
            )
            .await,
            cut
        );
        finish_sheet(&client, &run).await;
        assert_eq!(
            business(&client, &run, "sheet_trace", json!({"subject":"LOT-AL-2"})).await["ledger"]
                [0]["machine"],
            "LASER-B"
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p7_03_harness_sample_rework_and_terminal_genealogy() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_options(&dir.path().join("runs.sqlite3"), "0", true, true).await;
    timeout(WAIT,async {
        let run=open(&client,"wire-harness").await;
        for (serial,terminal) in [("WIRE-01","TERM-ALT"),("WIRE-02","TERM-STD")] {
            assert_eq!(shop_action(&client,&run,"harness_cut",&format!("{serial}-cut"),json!({"serial":serial})).await["phase"],"cut");
            assert_eq!(shop_action(&client,&run,"harness_crimp",&format!("{serial}-crimp"),json!({"serial":serial,"terminal_batch":terminal})).await["phase"],"crimped");
        }
        assert_eq!(shop_action(&client,&run,"harness_test","baseline",json!({"serial":"WIRE-01","result":"pass"})).await["phase"],"passed");
        advance(&client,&run,10).await;
        let event=delivered(&client,&run).await["events"][0].clone();
        assert_eq!(event["kind"],"harness_sample_failed");
        assert_eq!(shop_action(&client,&run,"harness_sample","sample",json!({"event_cursor":event["cursor"]})).await["affected"],json!(["WIRE-01"]));
        assert_eq!(shop_action(&client,&run,"harness_release","early",json!({"serial":"WIRE-01"})).await["code"],"TEST_PASS_REQUIRED");
        assert_eq!(shop_action(&client,&run,"harness_test","clean-test",json!({"serial":"WIRE-02","result":"pass"})).await["phase"],"passed");
        assert_eq!(shop_action(&client,&run,"harness_release","clean-release",json!({"serial":"WIRE-02"})).await["phase"],"released");
        assert_eq!(shop_action(&client,&run,"harness_rework","reject",json!({"serial":"WIRE-01","terminal_batch":"TERM-ALT","reason":"hold"})).await["code"],"TERMINAL_QUALITY_HOLD");
        assert_eq!(shop_action(&client,&run,"harness_rework","rework",json!({"serial":"WIRE-01","terminal_batch":"TERM-STD","reason":"replace suspect terminal"})).await["phase"],"reworked");
        assert_eq!(shop_action(&client,&run,"harness_release","early",json!({"serial":"WIRE-01"})).await["code"],"TEST_PASS_REQUIRED");
        assert_eq!(shop_action(&client,&run,"harness_test","retest",json!({"serial":"WIRE-01","result":"pass"})).await["phase"],"passed");
        assert_eq!(shop_action(&client,&run,"harness_release","finish",json!({"serial":"WIRE-01"})).await["phase"],"released");
        let history=business(&client,&run,"harness_trace",json!({"subject":"WIRE-01"})).await;
        let records=history["units"]["WIRE-01"]["history"].as_array().unwrap();
        assert!(records.iter().any(|record|record["operation"]=="crimp" && record["terminal_batch"]=="TERM-ALT" &&
            record["qualification"]=="QA-TERMINAL-12"));
        assert!(records.iter().any(|record|record["operation"]=="rework" && record["previous_terminal"]=="TERM-ALT" &&
            record["terminal_batch"]=="TERM-STD" && record["process_revision"]=="HARNESS-2"));
        for terminal in ["TERM-ALT","TERM-STD"] {
            assert!(business(&client,&run,"harness_trace",json!({"subject":terminal})).await["units"].get("WIRE-01").is_some());
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p7_04_five_industry_catalog_and_replay() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_options(&dir.path().join("runs.sqlite3"), "0", true, true).await;
    timeout(WAIT, async {
        let mut counts = Vec::new();
        let mut business_tools = std::collections::BTreeSet::new();
        for (scenario, version, count, tool) in [
            ("cnc", 2, 16, "mes_status"),
            ("smt", 2, 13, "smt_trace"),
            ("assembly", 2, 11, "assembly_kit"),
            ("sheet-metal", 1, 7, "sheet_trace"),
            ("wire-harness", 1, 7, "harness_trace"),
        ] {
            let mut traces = Vec::new();
            for _ in 0..2 {
                let opened = call(
                    &client,
                    "open_scenario",
                    json!({"scenario_id":scenario,"version":version,"seed":42}),
                )
                .await;
                assert_eq!(opened["ok"], true);
                let run = &opened["run_id"];
                let found = call(&client, "search_tools", json!({"run_id":run})).await;
                let tools = found["tools"].as_array().unwrap();
                assert_eq!(tools.len(), count);
                assert!(tools.iter().any(|entry| entry["name"] == tool));
                for entry in tools {
                    business_tools.insert(entry["name"].as_str().unwrap().to_owned());
                }
                advance(&client, run, 10).await;
                traces.push(delivered(&client, run).await);
                assert_eq!(
                    call(&client, "inspect_run", json!({"run_id":run})).await["replay_verified"],
                    true
                );
                assert_eq!(
                    call(&client, "close_run", json!({"run_id":run})).await["closed"],
                    true
                );
            }
            assert_eq!(traces[0], traces[1], "{scenario}");
            counts.push(json!({"scenario":scenario,"version":version,"tools":count}));
        }
        assert_eq!(business_tools.len(), 52);
        let (agent,_child)=connect_mode(&dir.path().join("runs.sqlite3"),"0",true).await;
        let methods: Vec<_> = agent.peer().list_tools(None).await.unwrap().tools.into_iter()
            .filter(|tool| !matches!(tool.name.as_ref(),"get_probe"|"set_probe"))
            .map(|tool|tool.name).collect();
        assert_eq!(methods.len(),8);
        assert!(methods.iter().any(|name|name=="read_events"));
        assert!(!methods.iter().any(|name|name=="inspect_run"));
        write_acceptance_evidence(
            "five-industry-catalog.json",
            json!({"scenarios":counts,"unique_business_tools":business_tools.len(),"agent_mcp_methods":methods}),
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p8_01_partial_reporting_reversal_and_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT,async {
        let run=cnc(&client).await;
        ready_for_report(&client,&run,"WO-1").await;
        let request=json!({"operation":"machining","quantity":4});
        let first=mes(&client,&run,"report_operation","WO-1","first",request.clone()).await;
        assert_eq!(first["remaining"],6);
        assert_eq!(first["report"]["serial_numbers"],json!(["WO-1-01","WO-1-02","WO-1-03","WO-1-04"]));
        assert_eq!(mes_status(&client,&run).await["state"]["phase"],"running");
        assert_eq!(mes(&client,&run,"report_operation","WO-1","first",request).await,first);
        let before=mes_status(&client,&run).await;
        assert_eq!(before["on_hand"],6);
        let out_of_sequence=mes(&client,&run,"dispatch_operation","WO-1","skip",json!({"operation":"assembly"})).await;
        assert_eq!(out_of_sequence["code"],"OPERATION_OUT_OF_SEQUENCE");
        let too_many=mes(&client,&run,"report_operation","WO-1","too-many",json!({"operation":"machining","quantity":7})).await;
        assert_eq!(too_many,json!({"ok":false,"code":"REPORT_EXCEEDS_REMAINING","remaining":6}));
        assert_eq!(mes_status(&client,&run).await,before);

        let second=mes(&client,&run,"report_operation","WO-1","second",json!({"operation":"machining","quantity":2})).await;
        assert_eq!(second["reported_quantity"],6);
        let blocked=mes(&client,&run,"reverse_report","WO-1","old",json!({"report_id":first["report"]["id"],"reason":"correction"})).await;
        assert_eq!(blocked["code"],"REPORT_HAS_DEPENDENCIES");
        let reverse_args=json!({"report_id":second["report"]["id"],"reason":"entry error"});
        let reversed=mes(&client,&run,"reverse_report","WO-1","reverse-second",reverse_args.clone()).await;
        assert_eq!(reversed["state"]["units"].as_array().unwrap().len(),4);
        assert_eq!(mes(&client,&run,"reverse_report","WO-1","reverse-second",reverse_args).await,reversed);
        assert_eq!(mes_status(&client,&run).await["on_hand"],6);
        assert_eq!(business(&client,&run,"trace",json!({"subject":"WO-1-05"})).await["links"],json!([]));

        let completed=mes(&client,&run,"report_operation","WO-1","remaining",json!({"operation":"machining"})).await;
        assert_eq!(completed["remaining"],0);
        assert_eq!(completed["report"]["serial_numbers"][0],"WO-1-07");
        let state=mes_status(&client,&run).await;
        assert_eq!(state["state"]["phase"],"ready");
        assert_eq!(state["on_hand"],0);
        assert_eq!(state["ledger"].as_array().unwrap().iter().map(|entry|entry["delta"].as_i64().unwrap()).sum::<i64>(),-10);
        assert_eq!(state["reports"][1]["reversed"],true);
        assert_eq!(business(&client,&run,"trace",json!({"subject":"LOT-1"})).await["links"][0]["serial_numbers"].as_array().unwrap().len(),10);
        assert_eq!(mes(&client,&run,"dispatch_operation","WO-1","assembly-dispatch",json!({"operation":"assembly"})).await["ok"],true);
        assert_eq!(call(&client,"close_run",json!({"run_id":run})).await["closed"],true);
    }).await.unwrap();
}

#[tokio::test]
async fn p8_02_public_mcp_contract_excludes_evaluation_internals() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let (agent, _a) = connect_mode(&db, "0", true).await;
    let (evaluation, _b) = connect_options(&db, "0", true, true).await;
    timeout(WAIT, async {
        let tools = agent.peer().list_tools(None).await.unwrap();
        assert_eq!(tools.tools.len(), 8);
        let opening = tools
            .tools
            .iter()
            .find(|tool| tool.name == "open_scenario")
            .unwrap();
        assert!(
            !opening.input_schema["properties"]
                .as_object()
                .unwrap()
                .contains_key("seed")
        );
        let scenarios = call(&agent, "list_scenarios", json!({})).await;
        let opened = call(
            &agent,
            "open_scenario",
            json!({"scenario_id":"cnc","version":2}),
        )
        .await;
        assert_eq!(opened["ok"], true);
        let run = &opened["run_id"];
        let view = call(&agent, "get_run", json!({"run_id":run})).await;
        for public in [
            serde_json::to_value(tools).unwrap(),
            scenarios,
            opened.clone(),
            view,
        ] {
            let serialized = public.to_string();
            for forbidden in [
                "mock",
                "fixture",
                "model_version",
                "run_parameters",
                "diagnosis",
                "inspect_run",
                "get_probe",
                "set_probe",
                "seed",
            ] {
                assert!(
                    !serialized.contains(forbidden),
                    "{forbidden} in {serialized}"
                );
            }
        }
        assert_eq!(
            call(&agent, "inspect_run", json!({"run_id":run})).await["code"],
            "UNKNOWN_TOOL"
        );
        assert_eq!(
            call(&agent, "get_probe", json!({})).await["code"],
            "UNKNOWN_TOOL"
        );
        assert_eq!(
            call(
                &agent,
                "open_scenario",
                json!({"scenario_id":"cnc","version":2,"seed":42})
            )
            .await["code"],
            "INVALID_ARGUMENT"
        );
        let legacy = open(&agent, "smt").await;
        assert_eq!(advance(&evaluation, &legacy, 1).await["event_cursor"], 1);
        assert_eq!(
            delivered(&evaluation, &legacy).await["events"][0]["kind"],
            "fixture_ready"
        );
        let public_events = delivered(&agent, &legacy).await;
        assert_eq!(public_events["events"][0]["kind"], "station_ready");
        assert_eq!(public_events["event_cursor"], 1);
        let controlled = call(
            &evaluation,
            "open_scenario",
            json!({"scenario_id":"cnc","version":2,"seed":42}),
        )
        .await;
        let evidence = call(
            &evaluation,
            "inspect_run",
            json!({"run_id":controlled["run_id"]}),
        )
        .await;
        assert_eq!(evidence["seed"], "42");
        assert_eq!(evidence["diagnosis"]["root_cause"], "spindle_wear");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p8b_01_public_order_reads() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        let mut contracts = Vec::new();
        for (scenario, first, second) in [("cnc", "WO-1", "WO-2"), ("assembly", "WO-E", "WO-L")] {
            let opened = call(&client, "open_scenario", json!({"scenario_id":scenario,"version":3})).await;
            assert_eq!(opened["ok"], true, "{opened}");
            let run = &opened["run_id"];
            let invoke = |name: &str, arguments| call(&client, "call_tool",
                json!({"run_id":run,"name":name,"arguments":arguments}));
            let search = call(&client, "search_tools", json!({"run_id":run})).await;
            assert!(search["tools"].as_array().unwrap().iter().any(|entry| entry["name"] == "list_work_orders"));
            contracts.push(call(&client, "describe_tool", json!({"run_id":run,"name":"list_work_orders"})).await);
            let page = invoke("list_work_orders", json!({"limit":1})).await;
            assert_eq!(page["orders"][0]["work_order"], first);
            assert_eq!(page["next_after_work_order"], first);
            let next = invoke("list_work_orders", json!({"after_work_order":first})).await;
            assert_eq!(next["orders"][0]["work_order"], second);
            assert!(next["next_after_work_order"].is_null());
            let detail = invoke("get_work_order", json!({"work_order":first})).await;
            assert_eq!(detail["status"], page["orders"][0]["status"]);
            assert_eq!(invoke("list_work_orders", json!({"status":"not-a-status"})).await["orders"], json!([]));
            if scenario == "cnc" {
                let before = invoke("get_release_readiness", json!({"work_order":first})).await;
                assert_eq!(before["ready"], true);
                let repeated = invoke("get_release_readiness", json!({"work_order":first})).await;
                assert_eq!(before, repeated);
                let release = invoke("release_order", json!({"work_order":first,"idempotency_key":"release","expected_cursor":before["cursor"]})).await;
                assert_eq!(release["ok"], true, "{release}");
                let blocked = invoke("get_release_readiness", json!({"work_order":first})).await;
                assert_eq!(blocked["blocking"], "INVALID_STATE");
                assert_eq!(blocked["cursor"], release["cursor"]);
                let held = invoke("hold_material", json!({"batch":"LOT-1","reason":"incoming inspection","idempotency_key":"hold"})).await;
                assert_eq!(held["ok"], true, "{held}");
                let blocked = invoke("get_release_readiness", json!({"work_order":second})).await;
                assert_eq!(blocked["blocking"], "MATERIAL_HELD");
                assert_eq!(blocked["cursor"], held["cursor"]);
                let rejected = invoke("release_order", json!({"work_order":second,"idempotency_key":"blocked-release"})).await;
                assert_eq!(rejected["code"], blocked["blocking"]);
                assert_eq!(invoke("get_release_readiness", json!({"work_order":second})).await, blocked);
            }
        }
        assert_eq!(contracts[0]["ok"], true);
        assert_eq!(contracts[0], contracts[1]);
    }).await.unwrap();
}

#[tokio::test]
async fn p8b_02_assembly_release_readiness() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        let mut contracts = Vec::new();
        for (scenario, version) in [("cnc", 3), ("assembly", 4), ("assembly-shortage", 2)] {
            let opened = call(
                &client,
                "open_scenario",
                json!({"scenario_id":scenario,"version":version}),
            )
            .await;
            assert_eq!(opened["ok"], true, "{opened}");
            let run = &opened["run_id"];
            contracts.push(
                call(
                    &client,
                    "describe_tool",
                    json!({"run_id":run,"name":"get_release_readiness"}),
                )
                .await,
            );
            if scenario == "cnc" {
                continue;
            }
            let invoke = |name: &str, arguments| {
                call(
                    &client,
                    "call_tool",
                    json!({"run_id":run,"name":name,"arguments":arguments}),
                )
            };
            let readiness = || invoke("get_release_readiness", json!({"work_order":"WO-E"}));
            let before = readiness().await;
            assert_eq!(before["eligible_cells"], json!(["CELL-A"]));
            assert_eq!(readiness().await, before);
            if scenario == "assembly-shortage" {
                let rejected = invoke(
                    "assembly_dispatch",
                    json!({"work_order":"WO-E","cell":"CELL-A","idempotency_key":"shortage"}),
                )
                .await;
                assert_eq!(before["ready"], false);
                assert_eq!(before["blocking"], "MATERIAL_SHORTAGE");
                assert_eq!(before["blocking"], rejected["code"]);
                assert_eq!(before["missing"], rejected["missing"]);
                assert_eq!(readiness().await, before);
                let replaced = invoke(
                    "assembly_substitute",
                    json!({"work_order":"WO-E","part":"DRIVE-ALT","idempotency_key":"substitute"}),
                )
                .await;
                assert_eq!(replaced["ok"], true, "{replaced}");
            }
            let ready = readiness().await;
            assert_eq!(ready["ready"], true);
            let rejected = invoke(
                "assembly_dispatch",
                json!({"work_order":"WO-E","cell":"CELL-B","idempotency_key":"wrong-cell"}),
            )
            .await;
            assert_eq!(rejected["code"], "CELL_NOT_CAPABLE");
            assert_eq!(readiness().await, ready);
            let dispatched = invoke(
                "assembly_dispatch",
                json!({"work_order":"WO-E","cell":"CELL-A","idempotency_key":"dispatch"}),
            )
            .await;
            assert_eq!(dispatched["ok"], true, "{dispatched}");
            let after = readiness().await;
            assert_eq!(after["blocking"], "INVALID_STATE");
            assert_eq!(after["cursor"], dispatched["cursor"]);
            assert_eq!(after["cursor"], ready["cursor"].as_i64().unwrap() + 1);
        }
        assert_eq!(contracts[0]["ok"], true);
        assert_eq!(contracts[0], contracts[1]);
        assert_eq!(contracts[0], contracts[2]);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p8b_03_shared_release_command() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        let mut contracts = Vec::new();
        for (scenario, version, order) in [("cnc", 4, "WO-1"), ("assembly-shortage", 3, "WO-E")] {
            let opened = call(&client, "open_scenario", json!({"scenario_id":scenario,"version":version})).await;
            assert_eq!(opened["ok"], true, "{opened}");
            let run = &opened["run_id"];
            let invoke = |name: &str, arguments| call(&client, "call_tool",
                json!({"run_id":run,"name":name,"arguments":arguments}));
            contracts.push(call(&client, "describe_tool", json!({"run_id":run,"name":"release_work_order"})).await);
            let readiness = || invoke("get_release_readiness", json!({"work_order":order}));
            let submit = |cursor: Value| json!({"work_order":order,"idempotency_key":"release","expected_cursor":cursor});
            let before = readiness().await;
            if scenario == "assembly-shortage" {
                let rejected = invoke("release_work_order", submit(before["cursor"].clone())).await;
                assert_eq!(rejected["code"], "MATERIAL_SHORTAGE");
                assert_eq!(readiness().await, before);
                let early = invoke("assembly_dispatch", json!({"work_order":order,"cell":"CELL-A","idempotency_key":"dispatch"})).await;
                assert_eq!(early["code"], "INVALID_STATE");
                assert_eq!(invoke("assembly_substitute", json!({"work_order":order,"part":"DRIVE-ALT","idempotency_key":"substitute"})).await["ok"], true);
            }
            let ready = readiness().await;
            assert_eq!(ready["ready"], true);
            let cursor = ready["cursor"].as_i64().unwrap();
            let stale = invoke("release_work_order", submit(json!(cursor + 1))).await;
            assert_eq!(stale["code"], "VERSION_CONFLICT");
            assert_eq!(readiness().await, ready);
            let request = submit(json!(cursor));
            let released = invoke("release_work_order", request.clone()).await;
            assert_eq!(released["status"], "released", "{released}");
            assert_eq!(released["cursor"], cursor + 1);
            assert_eq!(invoke("release_work_order", request).await, released);
            assert_eq!(invoke("release_work_order", submit(json!(cursor + 1))).await["code"], "IDEMPOTENCY_CONFLICT");
            assert_eq!(readiness().await["blocking"], "INVALID_STATE");
            if scenario == "cnc" {
                assert_eq!(released["snapshot"]["route_version"], ready["route_version"]);
            } else {
                assert_eq!(released["snapshot"]["parts"]["DRIVE-ALT"], 1);
                assert_eq!(invoke("assembly_dispatch", json!({"work_order":order,"cell":"CELL-A","idempotency_key":"dispatch"})).await["ok"], true);
                assert_eq!(invoke("assembly_substitute", json!({"work_order":order,"part":"DRIVE-ALT","idempotency_key":"late"})).await["code"], "INVALID_STATE");
                let built = invoke("assembly_build", json!({"work_order":order,"idempotency_key":"build"})).await;
                assert_eq!(built["state"]["installed"], released["snapshot"]["parts"]);
            }
        }
        assert_eq!(contracts[0]["ok"], true);
        assert_eq!(contracts[0], contracts[1]);
    }).await.unwrap();
}

#[tokio::test]
async fn p8b_04_create_and_update_draft() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        for (scenario, version, template, serial) in [
            ("cnc", 5, "WO-1", None),
            ("assembly", 6, "WO-E", Some("UNIT-NEW")),
        ] {
            let opened = call(&client, "open_scenario", json!({"scenario_id":scenario,"version":version})).await;
            assert_eq!(opened["ok"], true, "{opened}");
            let run = &opened["run_id"];
            let invoke = |name: &str, arguments| call(&client, "call_tool",
                json!({"run_id":run,"name":name,"arguments":arguments}));
            let tools = call(&client, "search_tools", json!({"run_id":run})).await;
            assert!(tools["tools"].as_array().unwrap().iter().any(|entry| entry["name"] == "create_work_order"));
            let initial = invoke("release_work_order", json!({"work_order":template,"idempotency_key":"initial","expected_cursor":0})).await;
            assert_eq!(initial["status"], "released", "{initial}");
            let mut args = json!({"work_order":"WO-NEW","template_work_order":template,
                "source_demand":"Demand-1","expected_cursor":1,"idempotency_key":"create"});
            if let Some(serial) = serial {
                let mut duplicate = args.clone();
                duplicate["serial"] = json!("UNIT-E-01");
                assert_eq!(invoke("create_work_order",duplicate).await["code"],"SERIAL_IN_USE");
                args["serial"] = json!(serial);
            }
            let created = invoke("create_work_order",args.clone()).await;
            assert_eq!(created["cursor"],2, "{created}");
            assert_eq!(invoke("create_work_order",args.clone()).await,created);
            args["idempotency_key"] = json!("duplicate");
            args["expected_cursor"] = json!(2);
            assert_eq!(invoke("create_work_order",args).await["code"],"WORK_ORDER_EXISTS");
            let detail = invoke("get_work_order",json!({"work_order":"WO-NEW"})).await;
            assert_eq!(detail["source_demand"],"Demand-1");
            let listed = invoke("list_work_orders",json!({"status":"planned"})).await;
            assert!(listed["orders"].as_array().unwrap().iter().any(|order| order["work_order"]=="WO-NEW"));
            let mut update = json!({"work_order":"WO-NEW","source_demand":"Demand-2","expected_cursor":1,"idempotency_key":"update"});
            assert_eq!(invoke("update_draft_order",update.clone()).await["code"],"VERSION_CONFLICT");
            update["expected_cursor"] = json!(2);
            let updated = invoke("update_draft_order",update.clone()).await;
            assert_eq!(updated["cursor"],3,"{updated}");
            let release = invoke("release_work_order",json!({"work_order":"WO-NEW","expected_cursor":3,"idempotency_key":"release"})).await;
            assert_eq!(release["status"],"released","{release}");
            assert_eq!(invoke("update_draft_order",update.clone()).await,updated);
            update["expected_cursor"] = json!(4);
            update["idempotency_key"] = json!("late-update");
            assert_eq!(invoke("update_draft_order",update).await["code"],"INVALID_STATE");
            let (second, _other) = connect_mode(&db, "0", true).await;
            let persisted = call(&second,"call_tool",json!({"run_id":run,"name":"get_work_order","arguments":{"work_order":"WO-NEW"}})).await;
            assert_eq!(persisted["source_demand"],"Demand-2");
            assert_ne!(persisted["status"],"planned");
            let isolated = call(&second,"open_scenario",json!({"scenario_id":scenario,"version":version})).await;
            let absent = call(&second,"call_tool",json!({"run_id":isolated["run_id"],"name":"get_work_order","arguments":{"work_order":"WO-NEW"}})).await;
            assert_eq!(absent["code"],"WORK_ORDER_NOT_FOUND");
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p8b_05_batch_release_partial_success() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        for (scenario, version, first, second, second_result) in [
            ("cnc", 6, "WO-1", "WO-2", "released"),
            ("assembly", 7, "WO-E", "WO-L", "released"),
            ("assembly-shortage", 4, "WO-E", "WO-L", "MATERIAL_SHORTAGE"),
        ] {
            let opened = call(&client, "open_scenario", json!({"scenario_id":scenario,"version":version})).await;
            assert_eq!(opened["ok"], true, "{opened}");
            let run = &opened["run_id"];
            let invoke = |name: &str, arguments| call(&client, "call_tool",
                json!({"run_id":run,"name":name,"arguments":arguments}));
            let tools = call(&client, "search_tools", json!({"run_id":run})).await;
            assert!(tools["tools"].as_array().unwrap().iter().any(|entry| entry["name"] == "release_work_orders"));
            let initial = if scenario == "assembly-shortage" {
                let replaced = invoke("assembly_substitute", json!({"work_order":first,"part":"DRIVE-ALT","idempotency_key":"sub"})).await;
                assert_eq!(replaced["ok"], true, "{replaced}");
                1
            } else {0};
            let empty = invoke("release_work_orders", json!({"work_orders":[],"expected_cursor":initial,"idempotency_key":"bad"})).await;
            assert_eq!(empty["code"],"INVALID_ARGUMENT");
            let invalid = invoke("release_work_orders", json!({"work_orders":[first,42],"expected_cursor":initial,"idempotency_key":"bad"})).await;
            assert_eq!(invalid["code"],"INVALID_ARGUMENT");
            let request = json!({"work_orders":[first,"UNKNOWN",first,second],"expected_cursor":initial,"idempotency_key":"batch"});
            let mut stale = request.clone();
            stale["expected_cursor"] = json!(initial + 1);
            assert_eq!(invoke("release_work_orders",stale.clone()).await["code"],"VERSION_CONFLICT");
            let receipt = invoke("release_work_orders",request.clone()).await;
            assert_eq!(receipt["ok"],true, "{receipt}");
            let items = receipt["results"].as_array().unwrap();
            assert_eq!(items.len(),4);
            assert_eq!(items[0]["status"],"released");
            assert_eq!(items[0]["cursor"],initial + 1);
            assert_eq!(items[1]["code"],"WORK_ORDER_NOT_FOUND");
            assert_eq!(items[1]["work_order"],"UNKNOWN");
            assert_eq!(items[2]["code"],"DUPLICATE_WORK_ORDER");
            let last_cursor = if second_result == "released" {
                assert_eq!(items[3]["status"],second_result);
                initial + 2
            } else {
                assert_eq!(items[3]["code"],second_result);
                assert_eq!(invoke("get_work_order",json!({"work_order":second})).await["status"],"planned");
                initial + 1
            };
            assert_eq!(receipt["cursor"],last_cursor);
            assert_eq!(invoke("release_work_orders",request).await,receipt);
            assert_eq!(invoke("release_work_orders",stale).await["code"],"IDEMPOTENCY_CONFLICT");
            let all_rejected = invoke("release_work_orders",json!({"work_orders":[first],"expected_cursor":last_cursor,"idempotency_key":"again"})).await;
            assert_eq!(all_rejected["results"][0]["code"],"INVALID_STATE");
            assert_eq!(all_rejected["cursor"],last_cursor);
            assert_eq!(call(&client,"get_run",json!({"run_id":run})).await["cursor"],last_cursor);
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p8b_06_hold_and_resume_orders() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        for (scenario, version, order) in [("cnc", 7, "WO-1"), ("assembly", 8, "WO-E")] {
            let opened = call(&client, "open_scenario", json!({"scenario_id":scenario,"version":version})).await;
            assert_eq!(opened["ok"], true, "{opened}");
            let run = &opened["run_id"];
            let invoke = |name: &str, arguments| call(&client, "call_tool",
                json!({"run_id":run,"name":name,"arguments":arguments}));
            let result = invoke("release_work_order",json!({"work_order":order,"expected_cursor":0,"idempotency_key":"release"})).await;
            assert_eq!(result["status"],"released");
            let (move_tool, move_args) = if scenario=="cnc" {
                ("dispatch_operation",json!({"work_order":order,"operation":"machining","idempotency_key":"dispatch"}))
            } else {
                ("assembly_dispatch",json!({"work_order":order,"cell":"CELL-A","idempotency_key":"dispatch"}))
            };
            let moved = invoke(move_tool,move_args).await;
            assert_eq!(moved["cursor"],2,"{moved}");
            let hold_args=json!({"work_order":order,"reason":"work review","expected_cursor":2,"idempotency_key":"hold"});
            let held=invoke("hold_work_order",hold_args.clone()).await;
            assert_eq!(held["status"],"held");
            assert_eq!(held["cursor"],3);
            let list = invoke("list_work_orders",json!({"status":"held"})).await;
            assert_eq!(list["orders"],json!([{"work_order":order,"status":"held"}]));
            let detail=invoke("get_work_order",json!({"work_order":order})).await;
            assert_eq!(detail["hold_reason"],"work review");
            let (blocked_tool, blocked_args) = if scenario=="cnc" {
                ("issue_material",json!({"work_order":order,"batch":"LOT-1","idempotency_key":"work"}))
            } else {
                ("assembly_build",json!({"work_order":order,"idempotency_key":"work"}))
            };
            assert_eq!(invoke(blocked_tool,blocked_args.clone()).await["code"],"WORK_ORDER_HELD");
            assert_eq!(invoke("hold_work_order",hold_args).await,held);
            let resume=|cursor|json!({"work_order":order,"expected_cursor":cursor,"idempotency_key":"resume"});
            assert_eq!(invoke("resume_work_order",resume(2)).await["code"],"VERSION_CONFLICT");
            let cursor=if scenario=="cnc" {
                assert_eq!(invoke("set_equipment",json!({"equipment_state":"down","idempotency_key":"down"})).await["cursor"],4);
                assert_eq!(invoke("resume_work_order",resume(4)).await["code"],"EQUIPMENT_UNAVAILABLE");
                assert_eq!(invoke("set_equipment",json!({"equipment_state":"available","idempotency_key":"up"})).await["cursor"],5);
                5
            } else {3};
            let resumed=invoke("resume_work_order",resume(cursor)).await;
            assert_eq!(resumed["status"],"dispatched","{resumed}");
            assert_eq!(resumed["cursor"],cursor+1);
            assert_eq!(invoke("resume_work_order",resume(cursor)).await,resumed);
            assert_eq!(invoke(blocked_tool,blocked_args).await["ok"],true);
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p8b_07_order_cancel_complete_close() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_options(&dir.path().join("runs.sqlite3"), "0", true, true).await;
    timeout(WAIT, async {
        for (scenario, version, order, unused) in [("cnc",8,"WO-1","WO-2"), ("assembly",9,"WO-E","WO-L")] {
            let opened=call(&client,"open_scenario",json!({"scenario_id":scenario,"version":version})).await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str, args| business(&client,run,name,args).await;
            let status=|| call(&client,"get_run",json!({"run_id":run}));
            let cancelled=invoke("cancel_work_order",json!({"work_order":unused,"expected_cursor":0,"idempotency_key":"cancel","reason":"demand withdrawn"})).await;
            assert_eq!(cancelled["status"],"cancelled","{cancelled}");
            let detail=invoke("get_work_order",json!({"work_order":unused})).await;
            assert_eq!(detail["cancel_reason"],"demand withdrawn");
            let released=invoke("release_work_order",json!({"work_order":order,"expected_cursor":1,"idempotency_key":"release"})).await;
            assert_eq!(released["status"],"released");
            let action=async |name:&str,key:&str, extra: Value| mes(&client,run,name,order,key,extra).await;
            if scenario=="cnc" {
                assert_eq!(action("dispatch_operation","dispatch1",json!({"operation":"machining"})).await["ok"],true);
                assert_eq!(action("issue_material","issue",json!({"batch":"LOT-1"})).await["ok"],true);
            } else {
                assert_eq!(action("assembly_dispatch","dispatch",json!({"cell":"CELL-A"})).await["ok"],true);
                assert_eq!(action("assembly_build","build",json!({})).await["ok"],true);
            }
            let cursor=status().await["cursor"].clone();
            let rejected=action("cancel_work_order","committed-cancel",json!({"expected_cursor":cursor,"reason":"late cancellation"})).await;
            assert_eq!(rejected["code"],"ORDER_HAS_EXECUTION");
            assert_eq!(status().await["cursor"],cursor);
            if scenario=="cnc" {
                for (operation,n) in [("machining",1),("assembly",2)] {
                    if n==2 {
                        assert_eq!(action("dispatch_operation","dispatch2",json!({"operation":operation})).await["ok"],true);
                    }
                    assert_eq!(action("start_operation",&format!("start{n}"),json!({"operation":operation})).await["ok"],true);
                    assert_eq!(action("report_operation",&format!("report{n}"),json!({"operation":operation})).await["ok"],true);
                }
                assert_eq!(action("dispatch_operation","dispatch3",json!({"operation":"inspection"})).await["ok"],true);
                assert_eq!(action("inspect_units","inspect",json!({"result":"fail"})).await["ok"],true);
                let cursor=status().await["cursor"].clone();
                assert_eq!(action("complete_work_order","complete",json!({"expected_cursor":cursor})).await["code"],"QUALITY_HOLD");
                assert_eq!(action("disposition","review",json!({"result":"pass","reason":"accepted after reinspection"})).await["ok"],true);
            } else {
                assert_eq!(action("assembly_start_test","start-test",json!({})).await["ok"],true);
                advance(&client,run,10).await;
                assert_eq!(action("assembly_finish_test","finish-test",json!({})).await["state"]["phase"],"passed");
            }
            let cursor=status().await["cursor"].as_i64().unwrap();
            assert_eq!(action("close_work_order","early-close",json!({"expected_cursor":cursor})).await["code"],"COMPLETION_REQUIRED");
            let completed=action("complete_work_order","complete",json!({"expected_cursor":cursor})).await;
            assert_eq!(completed["status"],"completed","{completed}");
            let mut cursor=cursor+1;
            if scenario=="assembly" {
                assert_eq!(action("close_work_order","early-close",json!({"expected_cursor":cursor})).await["code"],"DELIVERY_REQUIRED");
                assert_eq!(action("assembly_deliver","deliver",json!({})).await["ok"],true);
                cursor+=1;
            }
            let closed=action("close_work_order","close",json!({"expected_cursor":cursor})).await;
            assert_eq!(closed["status"],"closed","{closed}");
            assert_eq!(action("close_work_order","close",json!({"expected_cursor":cursor})).await,closed);
            let detail=invoke("get_work_order",json!({"work_order":order})).await;
            assert_eq!(detail["status"],"closed");
            assert_eq!(detail["completion_confirmed"],true);
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p8b_08_cross_shop_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        let opened=call(&client,"open_scenario",json!({"scenario_id":"cnc","version":9})).await;
        assert_eq!(opened["ok"],true,"{opened}");
        let run=&opened["run_id"];
        let action=async |name:&str,key:&str, extra: Value| mes(&client,run,name,"WO-1",key,extra).await;
        let released=action("release_work_order","release",json!({"expected_cursor":0})).await;
        let snapshot=&released["snapshot"];
        assert_eq!(snapshot["work_centers"],json!(["MACH-01","ASM-01","QA-01"]));
        let revised=business(&client,run,"revise_route",json!({"route_version":"route-2","work_centers":["MACH-01","ASM-02","QA-01"],"idempotency_key":"revision","expected_cursor":1})).await;
        assert_eq!(revised["cursor"],2,"{revised}");
        let same_version=business(&client,run,"revise_route",json!({"route_version":"route-1","idempotency_key":"old-route","expected_cursor":2})).await;
        assert_eq!(same_version["code"],"ROUTE_VERSION_EXISTS");
        let second=mes(&client,run,"release_work_order","WO-2","release2",json!({"expected_cursor":2})).await;
        assert_eq!(second["snapshot"]["route_version"],"route-2");
        assert_eq!(second["snapshot"]["work_centers"][1],"ASM-02");
        assert_eq!(action("dispatch_operation","skip",json!({"operation":"assembly","work_center":"ASM-01"})).await["code"],"OPERATION_OUT_OF_SEQUENCE");
        for (operation,center) in [("machining","MACH-01"),("assembly","ASM-01")] {
            if operation=="assembly" {
                let rejected=action("dispatch_operation","wrong-center",json!({"operation":operation,"work_center":"ASM-02"})).await;
                assert_eq!(rejected["code"],"WORK_CENTER_MISMATCH");
                assert_eq!(rejected["required_work_center"],center);
            }
            let dispatched=action("dispatch_operation",&format!("dispatch-{operation}"),json!({"operation":operation,"work_center":center})).await;
            assert_eq!(dispatched["ok"],true,"{dispatched}");
            if operation=="machining" {
                assert_eq!(action("issue_material","issue",json!({"batch":"LOT-1"})).await["ok"],true);
            }
            assert_eq!(action("start_operation",&format!("start-{operation}"),json!({"operation":operation})).await["ok"],true);
            assert_eq!(action("report_operation",&format!("report-{operation}"),json!({"operation":operation})).await["remaining"],0);
        }
        let detail=business(&client,run,"get_work_order",json!({"work_order":"WO-1"})).await;
        assert_eq!(detail["snapshot"],*snapshot);
        let status=mes_status(&client,run).await;
        let ledger:Vec<_>=status["ledger"].as_array().unwrap().iter().filter(|entry|entry["kind"]=="operation_dispatch").collect();
        assert_eq!(ledger.len(),2);
        assert_eq!(ledger[1]["work_center"],"ASM-01");
        assert_eq!(ledger[1]["route_version"],"route-1");
    }).await.unwrap();
}

#[tokio::test]
async fn p8c_01_operation_queries_and_queue() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        let mut contracts = Vec::new();
        for (scenario, order, first, second) in [
            ("cnc", "WO-1", "machining", "assembly"),
            ("assembly", "WO-E", "build", "final_test"),
        ] {
            let opened=call(&client,"open_scenario",json!({"scenario_id":scenario,"version":10})).await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str,args| business(&client,run,name,args).await;
            contracts.push(call(&client,"describe_tool",json!({"run_id":run,"name":"get_operation"})).await);
            assert_eq!(invoke("list_dispatch_queue",json!({})).await["operations"],json!([]));
            let read=|operation|json!({"work_order":order,"operation":operation});
            assert_eq!(invoke("get_operation",read(first)).await["operation"]["blocking"],"ORDER_NOT_RELEASED");
            assert_eq!(invoke("get_operation",read(second)).await["operation"]["blocking"],"PREDECESSOR_INCOMPLETE");
            assert_eq!(invoke("get_operation",read("unknown")).await["code"],"OPERATION_NOT_FOUND");
            let release=invoke("release_work_order",json!({"work_order":order,"expected_cursor":0,"idempotency_key":"release"})).await;
            assert_eq!(release["ok"],true,"{release}");
            let queue=invoke("list_dispatch_queue",json!({})).await;
            let detail=invoke("get_operation",read(first)).await;
            assert_eq!(queue["operations"],json!([detail["operation"]]));
            let order_ops=invoke("list_operations",json!({"work_order":order})).await;
            assert_eq!(order_ops["operations"][0],detail["operation"]);
            assert_eq!(detail["operation"]["status"],"ready");
            assert_eq!(call(&client,"get_run",json!({"run_id":run})).await["cursor"],1);
            if scenario=="cnc" {
                assert_eq!(detail["operation"]["work_center"],"MACH-01");
                let result=invoke("dispatch_operation",json!({"work_order":order,"operation":first,"work_center":"MACH-01","idempotency_key":"dispatch"})).await;
                assert_eq!(result["ok"],true);
                assert_eq!(invoke("list_dispatch_queue",json!({"work_center":"MACH-01"})).await["operations"],json!([]));
            } else {
                for (id, cell) in [(order,"CELL-A"),("WO-L","CELL-B")] {
                    if id != order {
                        assert_eq!(invoke("release_work_order",json!({"work_order":id,"expected_cursor":3,"idempotency_key":"release-other"})).await["ok"],true);
                    }
                    assert_eq!(invoke("assembly_dispatch",json!({"work_order":id,"cell":cell,"idempotency_key":format!("dispatch-{id}")})).await["ok"],true);
                    assert_eq!(invoke("assembly_build",json!({"work_order":id,"idempotency_key":format!("build-{id}")})).await["ok"],true);
                }
                let queue=invoke("list_dispatch_queue",json!({"work_center":"TEST-1"})).await;
                assert_eq!(queue["operations"][0]["work_order"],order);
                assert_eq!(queue["operations"][1]["blocking"],"TEST_PRIORITY_BLOCKED");
                assert_eq!(invoke("assembly_priority",json!({"work_order":"WO-L","priority":5,"idempotency_key":"priority"})).await["ok"],true);
                let queue=invoke("list_dispatch_queue",json!({"work_center":"TEST-1"})).await;
                assert_eq!(queue["operations"][0]["work_order"],"WO-L");
            }
        }
        assert_eq!(contracts[0]["ok"],true);
        assert_eq!(contracts[0],contracts[1]);
    }).await.unwrap();
}

#[tokio::test]
async fn p8c_02_resource_priority_and_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        let mut specs = Vec::new();
        for (scenario, first, second, operation, center, wrong) in [
            ("cnc","WO-1","WO-2","machining","MACH-01","ASM-01"),
            ("assembly","WO-E","WO-L","build","CELL-B","CELL-A"),
        ] {
            let opened=call(&client,"open_scenario",json!({"scenario_id":scenario,"version":11})).await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str,args| business(&client,run,name,args).await;
            specs.push(call(&client,"describe_tool",json!({"run_id":run,"name":"dispatch_operation"})).await);
            let capability=invoke("get_resource_capability",json!({"work_center":center})).await;
            assert_eq!(capability["resource"]["operations"],json!([operation]));
            assert_eq!(capability["resource"]["available"],true);
            assert_eq!(invoke("get_resource_capability",json!({"work_center":"UNKNOWN"})).await["code"],"RESOURCE_NOT_FOUND");
            let batch=invoke("release_work_orders",json!({"work_orders":[first,second],"expected_cursor":0,"idempotency_key":"release"})).await;
            assert_eq!(batch["cursor"],2);
            let priority=json!({"work_order":second,"priority":8,"expected_cursor":2,"idempotency_key":"priority"});
            let prioritized=invoke("set_dispatch_priority",priority.clone()).await;
            assert_eq!(prioritized["priority"],8,"{prioritized}");
            assert_eq!(invoke("set_dispatch_priority",priority).await,prioritized);
            let queue=invoke("list_dispatch_queue",json!({})).await;
            assert_eq!(queue["operations"][0]["work_order"],second);
            let dispatch=|resource, cursor|json!({"work_order":second,"operation":operation,"work_center":resource,"expected_cursor":cursor,"idempotency_key":"dispatch"});
            let rejected=invoke("dispatch_operation",dispatch(wrong,3)).await;
            assert_eq!(rejected["code"],if scenario=="cnc" {"WORK_CENTER_MISMATCH"} else {"CELL_NOT_CAPABLE"});
            let stale=invoke("dispatch_operation",dispatch(center,2)).await;
            assert_eq!(stale["code"],"VERSION_CONFLICT");
            let mut cursor=3;
            if scenario=="cnc" {
                assert_eq!(invoke("set_equipment",json!({"equipment_state":"down","idempotency_key":"down"})).await["cursor"],4);
                assert_eq!(invoke("get_resource_capability",json!({"work_center":center})).await["resource"]["available"],false);
                assert_eq!(invoke("dispatch_operation",dispatch(center,4)).await["code"],"EQUIPMENT_UNAVAILABLE");
                assert_eq!(invoke("set_equipment",json!({"equipment_state":"available","idempotency_key":"up"})).await["cursor"],5);
                cursor=5;
            }
            let dispatched=invoke("dispatch_operation",dispatch(center,cursor)).await;
            assert_eq!(dispatched["cursor"],cursor+1,"{dispatched}");
            assert_eq!(invoke("dispatch_operation",dispatch(center,cursor)).await,dispatched);
            let detail=invoke("get_operation",json!({"work_order":second,"operation":operation})).await;
            assert_eq!(detail["operation"]["status"],"dispatched");
            assert_eq!(detail["operation"]["work_center"],center);
        }
        assert_eq!(specs[0],specs[1]);
    }).await.unwrap();
}

#[tokio::test]
async fn p8c_03_exclusive_resource_and_reassignment() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    let (other, _other_child) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        for (scenario, first, second, operation, center, alternate, wrong) in [
            ("cnc","WO-1","WO-2","machining","MACH-01","MACH-02","ASM-01"),
            ("assembly","WO-E","WO-L","build","CELL-A","CELL-C","TEST-1"),
        ] {
            let opened=call(&client,"open_scenario",json!({"scenario_id":scenario,"version":12})).await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str,args| business(&client,run,name,args).await;
            let dispatch=|id, resource, cursor, key|json!({"work_order":id,"operation":operation,"work_center":resource,"expected_cursor":cursor,"idempotency_key":key});
            let batch=invoke("release_work_orders",json!({"work_orders":[first,second],"expected_cursor":0,"idempotency_key":"release"})).await;
            assert_eq!(batch["cursor"],2);
            let (a,b)=tokio::join!(
                invoke("dispatch_operation",dispatch(first,center,2,"first")),
                business(&other,run,"dispatch_operation",dispatch(second,center,2,"second"))
            );
            let (owner,waiting,rejected)=if a["ok"]==true {(first,second,b)} else {(second,first,a)};
            assert_eq!(rejected["code"],"VERSION_CONFLICT");
            let busy=invoke("dispatch_operation",dispatch(waiting,center,3,"busy")).await;
            assert_eq!(busy["code"],"RESOURCE_BUSY");
            assert_eq!(busy["occupied_by"],owner);
            let blocked=invoke("reassign_operation",dispatch(owner,wrong,3,"wrong")).await;
            assert_eq!(blocked["ok"],false);
            let resource=invoke("get_resource_capability",json!({"work_center":center})).await;
            assert_eq!(resource["resource"]["occupied_by"],owner);
            let reassigned=invoke("reassign_operation",dispatch(owner,alternate,3,"move")).await;
            assert_eq!(reassigned["cursor"],4,"{reassigned}");
            assert_eq!(invoke("reassign_operation",dispatch(owner,alternate,3,"move")).await,reassigned);
            assert_eq!(invoke("get_resource_capability",json!({"work_center":center})).await["resource"]["available"],true);
            assert_eq!(invoke("get_resource_capability",json!({"work_center":alternate})).await["resource"]["occupied_by"],owner);
            assert_eq!(invoke("dispatch_operation",dispatch(waiting,center,4,"busy")).await["cursor"],5);
            let rejected=invoke("reassign_operation",dispatch(waiting,alternate,5,"occupied-target")).await;
            assert_eq!(rejected["code"],"RESOURCE_BUSY");
            assert_eq!(invoke("get_resource_capability",json!({"work_center":center})).await["resource"]["occupied_by"],waiting);
            let cancelled=invoke("cancel_work_order",json!({"work_order":owner,"expected_cursor":5,"idempotency_key":"cancel","reason":"cancel before execution"})).await;
            assert_eq!(cancelled["status"],"cancelled");
            assert_eq!(invoke("get_resource_capability",json!({"work_center":alternate})).await["resource"]["available"],true);
            if scenario == "cnc" {
                assert_eq!(mes(&client,run,"issue_material",waiting,"issue",json!({"batch":"LOT-1"})).await["ok"],true);
                assert_eq!(mes(&client,run,"start_operation",waiting,"start",json!({"operation":operation})).await["ok"],true);
                assert_eq!(mes(&client,run,"report_operation",waiting,"partial",json!({"operation":operation,"quantity":4})).await["ok"],true);
                assert_eq!(invoke("get_resource_capability",json!({"work_center":center})).await["resource"]["occupied_by"],waiting);
                assert_eq!(mes(&client,run,"report_operation",waiting,"finish",json!({"operation":operation,"quantity":6})).await["remaining"],0);
            } else {
                assert_eq!(mes(&client,run,"assembly_build",waiting,"build",json!({})).await["ok"],true);
            }
            assert_eq!(invoke("get_resource_capability",json!({"work_center":center})).await["resource"]["available"],true);
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p8c_04_operation_start_pause_resume() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        for (scenario,id,operation,center) in [("cnc","WO-1","machining","MACH-01"),("assembly","WO-E","build","CELL-A")] {
            let opened=call(&client,"open_scenario",json!({"scenario_id":scenario,"version":13})).await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str,args| business(&client,run,name,args).await;
            assert_eq!(invoke("release_work_order",json!({"work_order":id,"expected_cursor":0,"idempotency_key":"release"})).await["ok"],true);
            assert_eq!(invoke("dispatch_operation",json!({"work_order":id,"operation":operation,"work_center":center,"expected_cursor":1,"idempotency_key":"dispatch"})).await["ok"],true);
            let cursor=if scenario=="cnc" {
                assert_eq!(mes(&client,run,"issue_material",id,"issue",json!({"batch":"LOT-1"})).await["ok"],true);3
            } else {2};
            let write=|cursor,key|json!({"work_order":id,"operation":operation,"expected_cursor":cursor,"idempotency_key":key});
            assert_eq!(invoke("start_operation",write(cursor,"start")).await["ok"],true);
            let mut pause=write(cursor+1,"pause");
            pause["reason"]=json!("work review");
            let paused=invoke("pause_operation",pause.clone()).await;
            assert_eq!(paused["ok"],true,"{paused}");
            assert_eq!(invoke("pause_operation",pause).await,paused);
            let row=invoke("get_operation",json!({"work_order":id,"operation":operation})).await;
            assert_eq!(row["operation"]["status"],"paused");
            assert_eq!(invoke("get_resource_capability",json!({"work_center":center})).await["resource"]["occupied_by"],id);
            let finish=if scenario=="cnc" {"report_operation"} else {"assembly_build"};
            let mut actual=json!({"work_order":id,"idempotency_key":"finish"});
            if scenario=="cnc" { actual["operation"]=json!(operation); }
            assert_eq!(invoke(finish,actual.clone()).await["code"],"INVALID_STATE");
            assert_eq!(invoke("resume_operation",write(cursor+1,"resume")).await["code"],"VERSION_CONFLICT");
            assert_eq!(invoke("resume_operation",write(cursor+2,"resume")).await["ok"],true);
            assert_eq!(invoke(finish,actual).await["ok"],true);
            assert_eq!(invoke("get_resource_capability",json!({"work_center":center})).await["resource"]["available"],true);
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p8c_05_explicit_completion_and_report_reversal() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        let opened=call(&client,"open_scenario",json!({"scenario_id":"cnc","version":14})).await;
        assert_eq!(opened["ok"],true,"{opened}");
        let run=&opened["run_id"];
        let invoke=async |name:&str,args| business(&client,run,name,args).await;
        let op=async |name:&str,key:&str,args| mes(&client,run,name,"WO-1",key,args).await;
        assert_eq!(op("release_work_order","release",json!({"expected_cursor":0})).await["ok"],true);
        assert_eq!(op("dispatch_operation","dispatch",json!({"operation":"machining","work_center":"MACH-01","expected_cursor":1})).await["ok"],true);
        assert_eq!(op("issue_material","issue",json!({"batch":"LOT-1"})).await["ok"],true);
        assert_eq!(op("start_operation","start",json!({"operation":"machining","expected_cursor":3})).await["ok"],true);
        let complete=|cursor|json!({"operation":"machining","expected_cursor":cursor});
        assert_eq!(op("complete_operation","complete",complete(4)).await["code"],"OPERATION_QUANTITY_INCOMPLETE");
        let first=op("post_operation_report","first",json!({"operation":"machining","quantity":4,"expected_cursor":4})).await;
        assert_eq!(first["remaining"],6,"{first}");
        let second=op("post_operation_report","second",json!({"operation":"machining","quantity":6,"expected_cursor":5})).await;
        assert_eq!(second["remaining"],0,"{second}");
        let status=mes_status(&client,run).await;
        assert_eq!(status["available_actions"],json!(["complete_operation"]));
        assert_eq!(invoke("get_resource_capability",json!({"work_center":"MACH-01"})).await["resource"]["occupied_by"],"WO-1");
        assert_eq!(op("dispatch_operation","downstream",json!({"operation":"assembly","work_center":"ASM-01","expected_cursor":6})).await["code"],"OPERATION_OUT_OF_SEQUENCE");
        let reverse=|id: &Value,cursor|json!({"report_id":id,"reason":"correction","expected_cursor":cursor});
        assert_eq!(op("reverse_operation_report","old",reverse(&first["report"]["id"],6)).await["code"],"REPORT_HAS_DEPENDENCIES");
        let reversed=op("reverse_operation_report","reverse",reverse(&second["report"]["id"],6)).await;
        assert_eq!(reversed["reversed"],true,"{reversed}");
        assert_eq!(mes_status(&client,run).await["on_hand"],6);
        let reports=invoke("list_operation_reports",json!({"work_order":"WO-1","operation":"machining"})).await;
        assert_eq!(reports["reports"][1]["reversed"],true);
        assert_eq!(invoke("get_command_result",json!({"idempotency_key":"reverse"})).await["receipt"],reversed);
        assert_eq!(invoke("get_command_result",json!({"idempotency_key":"old"})).await["code"],"COMMAND_RESULT_NOT_FOUND");
        let replacement=op("post_operation_report","replacement",json!({"operation":"machining","quantity":6,"expected_cursor":7})).await;
        assert_eq!(replacement["report"]["serial_numbers"][0],"WO-1-11");
        let completed=op("complete_operation","complete",complete(8)).await;
        assert_eq!(completed["status"],"completed","{completed}");
        assert_eq!(op("complete_operation","complete",complete(8)).await,completed);
        assert_eq!(invoke("get_resource_capability",json!({"work_center":"MACH-01"})).await["resource"]["available"],true);
        assert_eq!(op("dispatch_operation","downstream",json!({"operation":"assembly","work_center":"ASM-01","expected_cursor":9})).await["ok"],true);
        assert_eq!(op("reverse_operation_report","late",reverse(&replacement["report"]["id"],10)).await["code"],"REPORT_HAS_DEPENDENCIES");
    }).await.unwrap();
}

#[tokio::test]
async fn p8c_06_assembly_actuals_and_test_dependency() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("runs.sqlite3"), "0", true).await;
    timeout(WAIT, async {
        let opened=call(&client,"open_scenario",json!({"scenario_id":"assembly","version":14})).await;
        assert_eq!(opened["ok"],true,"{opened}");
        let run=&opened["run_id"];
        let op=async |name:&str,key:&str,args| mes(&client,run,name,"WO-E",key,args).await;
        let invoke=async |name:&str,args| business(&client,run,name,args).await;
        let cnc=call(&client,"open_scenario",json!({"scenario_id":"cnc","version":14})).await;
        for name in ["post_operation_report","list_operation_reports","reverse_operation_report","complete_operation","get_command_result"] {
            let spec=call(&client,"describe_tool",json!({"run_id":run,"name":name})).await;
            assert_eq!(spec["ok"],true);
            assert_eq!(spec,call(&client,"describe_tool",json!({"run_id":cnc["run_id"],"name":name})).await);
        }
        assert_eq!(op("release_work_order","release",json!({"expected_cursor":0})).await["ok"],true);
        assert_eq!(op("dispatch_operation","dispatch",json!({"operation":"build","work_center":"CELL-A","expected_cursor":1})).await["ok"],true);
        assert_eq!(op("start_operation","start",json!({"operation":"build","expected_cursor":2})).await["ok"],true);
        assert_eq!(op("assembly_build","legacy",json!({})).await["code"],"TOOL_UNAVAILABLE");
        let post=|cursor,quantity|json!({"operation":"build","expected_cursor":cursor,"quantity":quantity});
        let complete=|cursor|json!({"operation":"build","expected_cursor":cursor});
        assert_eq!(op("complete_operation","complete",complete(3)).await["code"],"OPERATION_QUANTITY_INCOMPLETE");
        assert_eq!(op("post_operation_report","post",post(3,2)).await["code"],"REPORT_EXCEEDS_REMAINING");
        let initial = invoke("assembly_kit",json!({"work_order":"WO-E"})).await;
        let actual=op("post_operation_report","post",post(3,1)).await;
        assert_eq!(actual["remaining"],0,"{actual}");
        assert_eq!(actual["report"]["components"]["DRIVE-STD"],1);
        assert_eq!(op("post_operation_report","post",post(3,1)).await,actual);
        assert_eq!(op("assembly_start_test","test",json!({})).await["ok"],false);
        assert_eq!(invoke("get_resource_capability",json!({"work_center":"CELL-A"})).await["resource"]["occupied_by"],"WO-E");
        assert_eq!(op("complete_operation","complete",complete(4)).await["status"],"completed");
        assert_eq!(invoke("get_resource_capability",json!({"work_center":"CELL-A"})).await["resource"]["available"],true);
        let reverse=|id: &Value,cursor|json!({"report_id":id,"reason":"actual correction","expected_cursor":cursor});
        let reversed=op("reverse_operation_report","reverse",reverse(&actual["report"]["id"],5)).await;
        assert_eq!(reversed["reversed"],true,"{reversed}");
        assert_eq!(invoke("assembly_kit",json!({"work_order":"WO-E"})).await["stock"],initial["stock"]);
        let reports=invoke("list_operation_reports",json!({"work_order":"WO-E","operation":"build"})).await;
        assert_eq!(reports["reports"][0]["reversed"],true);
        assert_eq!(reports["reports"][0]["components"],actual["report"]["components"]);
        assert_eq!(invoke("get_resource_capability",json!({"work_center":"CELL-A"})).await["resource"]["occupied_by"],"WO-E");
        let corrected=op("post_operation_report","corrected",post(6,1)).await;
        assert_ne!(corrected["report"]["id"],actual["report"]["id"]);
        assert_eq!(invoke("get_command_result",json!({"idempotency_key":"corrected"})).await["receipt"],corrected);
        assert_eq!(op("complete_operation","complete-again",complete(7)).await["status"],"completed");
        assert_eq!(op("assembly_start_test","test",json!({})).await["ok"],true);
        assert_eq!(op("reverse_operation_report","late",reverse(&corrected["report"]["id"],9)).await["code"],"REPORT_HAS_DEPENDENCIES");
    }).await.unwrap();
}

#[tokio::test]
async fn p8c_07_des_stop_repair_and_replay() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runs.sqlite3");
    let (public, _public_child) = connect_mode(&db, "0", true).await;
    let (control, _control_child) = connect_options(&db, "0", true, true).await;
    timeout(WAIT, async {
        let opened = call(
            &public,
            "open_scenario",
            json!({"scenario_id":"cnc","version":15}),
        )
        .await;
        assert_eq!(opened["ok"], true, "{opened}");
        let run = &opened["run_id"];
        let op =
            async |name: &str, key: &str, args| mes(&public, run, name, "WO-1", key, args).await;
        assert_eq!(
            op(
                "release_work_order",
                "release",
                json!({"expected_cursor":0})
            )
            .await["ok"],
            true
        );
        assert_eq!(
            op(
                "dispatch_operation",
                "dispatch",
                json!({"operation":"machining","work_center":"MACH-01","expected_cursor":1})
            )
            .await["ok"],
            true
        );
        assert_eq!(
            op("issue_material", "issue", json!({"batch":"LOT-1"})).await["ok"],
            true
        );
        assert_eq!(
            op(
                "start_operation",
                "start",
                json!({"operation":"machining","expected_cursor":3})
            )
            .await["ok"],
            true
        );
        advance(&control, run, 3).await;
        let events = call(&public, "read_events", json!({"run_id":run})).await;
        assert!(
            events["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["kind"] == "cnc_equipment_stopped")
        );
        let before = mes_status(&public, run).await;
        assert_eq!(before["blocking"], "EQUIPMENT_UNAVAILABLE");
        assert_eq!(before["cursor"], 5);
        assert_eq!(
            op(
                "post_operation_report",
                "report",
                json!({"operation":"machining","quantity":1,"expected_cursor":5})
            )
            .await["code"],
            "EQUIPMENT_UNAVAILABLE"
        );
        assert_eq!(
            op(
                "pause_operation",
                "pause",
                json!({"operation":"machining","reason":"equipment stopped","expected_cursor":5})
            )
            .await["ok"],
            true
        );
        let maintenance =
            |cursor, key| json!({"asset":"CNC-1","expected_cursor":cursor,"idempotency_key":key});
        let receipt = business(
            &public,
            run,
            "request_maintenance",
            maintenance(6, "repair"),
        )
        .await;
        assert_eq!(receipt["status"], "maintenance_requested", "{receipt}");
        assert_eq!(
            business(
                &public,
                run,
                "request_maintenance",
                maintenance(6, "repair")
            )
            .await,
            receipt
        );
        assert_eq!(
            business(
                &public,
                run,
                "request_maintenance",
                maintenance(7, "duplicate")
            )
            .await["code"],
            "MAINTENANCE_PENDING"
        );
        let resume = |cursor| json!({"operation":"machining","expected_cursor":cursor});
        assert_eq!(
            op("resume_operation", "resume", resume(7)).await["code"],
            "EQUIPMENT_UNAVAILABLE"
        );
        advance(&control, run, 2).await;
        let recovered = mes_status(&public, run).await;
        assert_eq!(recovered["equipment_down"], false);
        assert_eq!(recovered["cursor"], 8);
        assert_eq!(
            op("resume_operation", "resume", resume(8)).await["ok"],
            true
        );
        assert_eq!(
            op(
                "post_operation_report",
                "report",
                json!({"operation":"machining","quantity":1,"expected_cursor":9})
            )
            .await["remaining"],
            9
        );
        let replay = call(&control, "inspect_run", json!({"run_id":run})).await;
        assert_eq!(replay["replay_verified"], true, "{replay}");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p8d_01_public_inspection_registry() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("quality.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        for (scenario, version, order) in [("cnc", 16, "WO-1"), ("assembly", 15, "WO-E")] {
            let open_run = async |version| {
                let response = call(
                    &client,
                    "open_scenario",
                    json!({"scenario_id":scenario,"version":version}),
                )
                .await;
                assert_eq!(response["ok"], true, "{response}");
                response["run_id"].clone()
            };
            let run = open_run(version).await;
            let discovered = call(
                &client,
                "search_tools",
                json!({"run_id":run,"query":"inspection"}),
            )
            .await;
            for name in ["create_inspection", "list_inspections", "get_inspection"] {
                assert!(
                    discovered["tools"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|tool| tool["name"] == name),
                    "{discovered}"
                );
                let described =
                    call(&client, "describe_tool", json!({"run_id":run,"name":name})).await;
                assert_eq!(described["tool"]["domain"], "quality", "{described}");
            }
            let create = async |id: &str, key: &str, expected, work_order: &str| {
                business(
                    &client,
                    &run,
                    "create_inspection",
                    json!({"inspection_id":id,
                    "work_order":work_order,"specification_version":"QA-DIM@2",
                    "idempotency_key":key,"expected_cursor":expected}),
                )
                .await
            };
            assert_eq!(
                create("QC-1", "create", 0, "missing").await["code"],
                "WORK_ORDER_NOT_FOUND"
            );
            let (peer, _peer_child) = connect_mode(&db, "0", true).await;
            let (first, concurrent) = tokio::join!(
                create("QC-1", "create", 0, order),
                business(
                    &peer,
                    &run,
                    "create_inspection",
                    json!({"inspection_id":"QC-1",
                    "work_order":order,"specification_version":"QA-DIM@2",
                    "idempotency_key":"create","expected_cursor":0})
                ),
            );
            assert_eq!(first["cursor"], 1, "{first}");
            assert_eq!(first, concurrent);
            assert_eq!(
                first["inspection"],
                json!({"inspection_id":"QC-1","work_order":order,
                "specification_version":"QA-DIM@2","status":"pending"})
            );
            assert_eq!(create("QC-1", "create", 0, order).await, first);
            assert_eq!(
                create("QC-2", "create", 1, order).await["code"],
                "IDEMPOTENCY_CONFLICT"
            );
            assert_eq!(
                create("QC-2", "create-2", 0, order).await["code"],
                "VERSION_CONFLICT"
            );
            assert_eq!(
                create("QC-1", "duplicate", 1, order).await["code"],
                "INSPECTION_ALREADY_EXISTS"
            );
            assert_eq!(create("QC-2", "create-2", 1, order).await["cursor"], 2);
            let page = business(
                &client,
                &run,
                "list_inspections",
                json!({"work_order":order,"limit":1}),
            )
            .await;
            assert_eq!(page["inspections"], json!([first["inspection"]]));
            assert_eq!(page["next_after_inspection_id"], "QC-1");
            let next = business(
                &client,
                &run,
                "list_inspections",
                json!({"limit":1,"after_inspection_id":page["next_after_inspection_id"]}),
            )
            .await;
            assert_eq!(next["inspections"][0]["inspection_id"], "QC-2");
            assert!(next["next_after_inspection_id"].is_null());

            let (reconnected, _child) = connect_mode(&db, "0", true).await;
            let get = json!({"inspection_id":"QC-1"});
            let saved = business(&reconnected, &run, "get_inspection", get.clone()).await;
            assert_eq!(saved["inspection"], first["inspection"]);
            assert_eq!(
                business(
                    &reconnected,
                    &run,
                    "get_command_result",
                    json!({"idempotency_key":"create"})
                )
                .await["receipt"],
                first
            );
            assert_eq!(
                call(&client, "get_run", json!({"run_id":run})).await["cursor"],
                2
            );
            let isolated = open_run(version).await;
            assert_eq!(
                business(&client, &isolated, "get_inspection", get.clone()).await["code"],
                "INSPECTION_NOT_FOUND"
            );
            assert_eq!(
                business(&client, &isolated, "list_inspections", json!({})).await["inspections"],
                json!([])
            );
            let previous = open_run(version - 1).await;
            assert_eq!(
                business(&client, &previous, "get_inspection", get).await["code"],
                "TOOL_UNAVAILABLE"
            );
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn p8d_02_measurement_and_inspection_result() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("quality.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        for (scenario, version, order) in [("cnc", 17, "WO-1"), ("assembly", 16, "WO-E")] {
            let opened = call(&client, "open_scenario", json!({"scenario_id":scenario,"version":version})).await;
            assert_eq!(opened["ok"], true, "{opened}");
            let run = &opened["run_id"];
            let discovered = call(&client, "search_tools", json!({"run_id":run,"domain":"quality","effect":"write"})).await;
            for name in ["record_measurement", "complete_inspection"] {
                assert!(discovered["tools"].as_array().unwrap().iter().any(|tool| tool["name"] == name));
            }
            let invoke = async |name: &str, args| business(&client, run, name, args).await;
            let create = |id: &str, cursor| json!({"inspection_id":id,"work_order":order,
                "specification_version":"QA-DIM@2","idempotency_key":format!("create-{id}"),"expected_cursor":cursor});
            let measure = |id: &str, characteristic: &str, key: &str, value: f64, lower: f64, upper: f64, cursor| {
                json!({"inspection_id":id,"characteristic":characteristic,"unit":"mm","value":value,
                    "lower_limit":lower,"upper_limit":upper,"idempotency_key":key,"expected_cursor":cursor})
            };
            let complete = |id: &str, key: &str, cursor| json!({"inspection_id":id,"idempotency_key":key,"expected_cursor":cursor});
            assert_eq!(invoke("create_inspection", create("QC-1", 0)).await["cursor"], 1);
            assert_eq!(invoke("complete_inspection", complete("QC-1", "complete", 1)).await["code"], "MEASUREMENT_REQUIRED");
            assert_eq!(invoke("record_measurement", measure("QC-1", "width", "width", 4.5, 6.0, 4.0, 1)).await["field"], "upper_limit");
            let boundary_args = measure("QC-1", "width", "width", 4.5, 4.0, 4.5, 1);
            let boundary = invoke("record_measurement", boundary_args.clone()).await;
            assert_eq!(boundary["measurement"]["passed"], true, "{boundary}");
            assert_eq!(invoke("record_measurement", boundary_args.clone()).await, boundary);
            assert_eq!(invoke("record_measurement", measure("QC-1", "width", "retest", 4.3, 4.0, 4.5, 2)).await["code"], "MEASUREMENT_ALREADY_EXISTS");
            assert_eq!(invoke("record_measurement", measure("QC-1", "length", "length", 6.2, 5.0, 6.0, 1)).await["code"], "VERSION_CONFLICT");
            let over = invoke("record_measurement", measure("QC-1", "length", "length", 6.2, 5.0, 6.0, 2)).await;
            assert_eq!(over["measurement"]["passed"], false, "{over}");
            let failed = invoke("complete_inspection", complete("QC-1", "complete", 3)).await;
            assert_eq!(failed["inspection"]["status"], "failed", "{failed}");
            assert_eq!(invoke("complete_inspection", complete("QC-1", "complete", 3)).await, failed);
            assert_eq!(invoke("record_measurement", boundary_args).await, boundary);
            assert_eq!(invoke("record_measurement", measure("QC-1", "height", "late", 4.2, 4.0, 4.5, 4)).await["code"], "INSPECTION_COMPLETED");
            assert_eq!(invoke("complete_inspection", complete("QC-1", "new-complete", 4)).await["code"], "INSPECTION_COMPLETED");
            assert_eq!(invoke("create_inspection", create("QC-2", 4)).await["cursor"], 5);
            assert_eq!(invoke("record_measurement", measure("QC-2", "width", "pass-width", 4.0, 4.0, 4.5, 5)).await["measurement"]["passed"], true);
            assert_eq!(invoke("complete_inspection", complete("QC-2", "pass-complete", 6)).await["inspection"]["status"], "passed");
            let (reconnected, _child) = connect_mode(&db, "0", true).await;
            let detail = business(&reconnected, run, "get_inspection", json!({"inspection_id":"QC-1"})).await;
            assert_eq!(detail["inspection"],failed["inspection"]);
            assert_eq!(detail["measurements"],json!([over["measurement"], boundary["measurement"]]));
            let summary = business(&reconnected, run, "list_inspections", json!({"work_order":order})).await;
            assert_eq!(summary["inspections"][0]["status"], "failed");
            assert_eq!(summary["inspections"][1]["status"], "passed");
            assert_eq!(call(&client, "get_run", json!({"run_id":run})).await["cursor"], 7);
            let other = call(&client, "open_scenario", json!({"scenario_id":scenario,"version":version})).await;
            assert_eq!(business(&client, &other["run_id"], "complete_inspection", complete("QC-1","complete",0)).await["code"], "INSPECTION_NOT_FOUND");
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p8d_03_nonconformance_hold_and_release() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("quality.sqlite3");
    let (client, _child) = connect_options(&db, "0", true, true).await;
    timeout(WAIT, async {
        for (scenario, version, order, other) in [("cnc", 18, "WO-1", "WO-2"), ("assembly", 17, "WO-E", "WO-L")] {
            let opened = call(&client, "open_scenario", json!({"scenario_id":scenario,"version":version})).await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run = &opened["run_id"];
            let invoke = async |name: &str, args| business(&client, run, name, args).await;
            let cursor = async || call(&client,"get_run",json!({"run_id":run})).await["cursor"].as_i64().unwrap();
            let op = async |name: &str, key: &str, mut args: Value| {
                args["expected_cursor"] = json!(cursor().await);
                mes(&client,run,name,order,key,args).await
            };
            let check = async |id: &str, value: f64, upper: f64| {
                let created = invoke("create_inspection",json!({"inspection_id":id,"work_order":order,
                    "specification_version":"QA-DIM@2","expected_cursor":cursor().await,"idempotency_key":format!("create-{id}")})).await;
                assert_eq!(created["ok"],true,"{created}");
                let measured = invoke("record_measurement",json!({"inspection_id":id,"characteristic":"width","value":value,
                    "unit":"mm","lower_limit":4.0,"upper_limit":upper,"expected_cursor":created["cursor"],"idempotency_key":format!("measure-{id}")})).await;
                assert_eq!(measured["ok"],true,"{measured}");
                invoke("complete_inspection",json!({"inspection_id":id,"expected_cursor":measured["cursor"],"idempotency_key":format!("finish-{id}")})).await
            };
            let transition = async |name: &str, id: &str, key: &str, extra: Value| {
                let mut args = extra;
                for (field,value) in [("inspection_id",json!(id)),("idempotency_key",json!(key)),
                    ("expected_cursor",json!(cursor().await)),("responsible",json!("QA-01")),
                    ("reason",json!("quality review")),("evidence",json!("QA-LOG-42"))] {
                    args[field]=value;
                }
                invoke(name,args).await
            };

            assert_eq!(op("release_work_order","release",json!({})).await["ok"],true);
            let operations = if scenario == "cnc" {
                vec![("machining","MACH-01",10),("assembly","ASM-01",10)]
            } else {
                vec![("build","CELL-A",1)]
            };
            for (operation,center,quantity) in operations {
                assert_eq!(op("dispatch_operation",&format!("dispatch-{operation}"),json!({"operation":operation,"work_center":center})).await["ok"],true);
                if operation=="machining" {
                    assert_eq!(mes(&client,run,"issue_material",order,"issue",json!({"batch":"LOT-1"})).await["ok"],true);
                }
                assert_eq!(op("start_operation",&format!("start-{operation}"),json!({"operation":operation})).await["ok"],true);
                let reported = op("post_operation_report",&format!("report-{operation}"),json!({"operation":operation,"quantity":quantity})).await;
                assert_eq!(reported["remaining"],0,"{reported}");
                assert_eq!(op("complete_operation",&format!("complete-{operation}"),json!({"operation":operation})).await["status"],"completed");
            }
            if scenario == "cnc" {
                let dispatched = op("dispatch_operation","dispatch-inspection",json!({"operation":"inspection","work_center":"QA-01"})).await;
                assert_eq!(dispatched["ok"],true,"{dispatched}");
            } else {
                assert_eq!(mes(&client,run,"assembly_start_test",order,"start-test",json!({})).await["ok"],true);
                advance(&client,run,10).await;
                assert_eq!(mes(&client,run,"assembly_finish_test",order,"finish-test",json!({})).await["state"]["phase"],"passed");
            }
            assert_eq!(check("old-pass",4.2,4.5).await["inspection"]["status"],"passed");
            assert_eq!(check("failure",4.7,4.5).await["inspection"]["status"],"failed");
            let gated = if scenario=="cnc" { "inspect_units" } else { "assembly_deliver" };
            let input = if scenario=="cnc" {json!({"result":"pass"})} else {json!({})};
            let blocked_at=cursor().await;
            assert_eq!(mes(&client,run,gated,order,"accept",input.clone()).await["code"],"QUALITY_HOLD");
            assert_eq!(op("complete_work_order","complete",json!({})).await["code"],"QUALITY_HOLD");
            assert_eq!(op("close_work_order","close",json!({})).await["code"],"QUALITY_HOLD");
            assert_eq!(cursor().await,blocked_at);
            let unaffected=invoke("close_work_order",json!({"work_order":other,"idempotency_key":"other-close","expected_cursor":cursor().await})).await;
            assert_eq!(unaffected["code"],"INVALID_STATE","{unaffected}");
            assert_eq!(transition("open_nonconformance","old-pass","reject-pass",json!({})).await["code"],"FAILED_INSPECTION_REQUIRED");
            assert_eq!(transition("open_nonconformance","failure","open",json!({})).await["nonconformance"]["status"],"open");
            assert_eq!(transition("place_quality_hold","failure","hold",json!({})).await["nonconformance"]["status"],"held");
            assert_eq!(transition("release_quality_hold","failure","release-hold",json!({})).await["code"],"NONCONFORMANCE_STATE_INVALID");
            let retest=|id:&str| json!({"decision":"reinspection","reinspection_id":id});
            assert_eq!(transition("disposition_nonconformance","failure","disposition",retest("old-pass")).await["code"],"REINSPECTION_NOT_ACCEPTABLE");
            assert_eq!(check("relaxed-pass",4.8,5.0).await["inspection"]["status"],"passed");
            assert_eq!(transition("disposition_nonconformance","failure","disposition",retest("relaxed-pass")).await["code"],"REINSPECTION_NOT_ACCEPTABLE");
            assert_eq!(check("new-pass",4.3,4.5).await["inspection"]["status"],"passed");
            assert_eq!(transition("disposition_nonconformance","failure","disposition",retest("new-pass")).await["nonconformance"]["status"],"disposed");
            assert_eq!(op("close_work_order","close",json!({})).await["code"],"QUALITY_HOLD");
            let release_args=json!({"inspection_id":"failure","idempotency_key":"release-hold",
                "expected_cursor":cursor().await,"responsible":"QA-01","reason":"quality review","evidence":"QA-LOG-42"});
            let released=invoke("release_quality_hold",release_args.clone()).await;
            assert_eq!(released["nonconformance"]["status"],"released","{released}");
            let (peer,_child)=connect_mode(&db,"0",true).await;
            assert_eq!(business(&peer,run,"release_quality_hold",release_args).await,released);
            let detail=business(&peer,run,"get_inspection",json!({"inspection_id":"failure"})).await;
            assert_eq!(detail["inspection"]["status"],"failed");
            assert_eq!(detail["nonconformance"],released["nonconformance"]);
            assert_eq!(detail["nonconformance"]["history"][2]["reinspection_id"],"new-pass");
            assert_eq!(detail["nonconformance"]["history"][2]["responsible"],"QA-01");
            assert_eq!(check("concession",4.8,4.5).await["inspection"]["status"],"failed");
            for (action, key, state, extra) in [
                ("open_nonconformance","open-c","open",json!({})),
                ("place_quality_hold","hold-c","held",json!({})),
                ("disposition_nonconformance","dispose-c","disposed",json!({"decision":"concession"})),
            ] {
                assert_eq!(transition(action,"concession",key,extra).await["nonconformance"]["status"],state);
            }
            assert_eq!(op("complete_work_order","complete",json!({})).await["code"],"QUALITY_HOLD");
            assert_eq!(transition("release_quality_hold","concession","release-c",json!({})).await["nonconformance"]["status"],"released");
            let accepted=mes(&client,run,gated,order,"accept",input).await;
            assert_eq!(accepted["ok"],true,"{accepted}");
            assert_eq!(op("complete_work_order","complete",json!({})).await["status"],"completed");
            assert_eq!(op("close_work_order","close",json!({})).await["status"],"closed");
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p9a_01_inspection_plan_revision_binding() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("quality.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        for (scenario, version, order) in [("cnc", 19, "WO-1"), ("assembly", 18, "WO-E")] {
            let open = async || call(&client,"open_scenario",json!({"scenario_id":scenario,"version":version})).await;
            let opened=open().await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str,args| business(&client,run,name,args).await;
            let create_plan = |revision, lower, upper, cursor| json!({"plan_id":"DIM","revision":revision,
                "characteristics":{"width":{"unit":"mm","lower_limit":lower,"upper_limit":upper},
                    "height":{"unit":"mm","lower_limit":1,"upper_limit":2}},
                "expected_cursor":cursor,"idempotency_key":format!("plan-{revision}")});
            let activate = |revision,cursor| json!({"plan_id":"DIM","revision":revision,"expected_cursor":cursor,
                "idempotency_key":format!("activate-{revision}")});
            let create = |id:&str,reference:&str,cursor| json!({"inspection_id":id,"work_order":order,
                "specification_version":reference,"expected_cursor":cursor,"idempotency_key":format!("inspection-{id}")});
            let measure = |id:&str,item:&str,value,unit:&str,lower,upper,cursor| json!({
                "inspection_id":id,"characteristic":item,"value":value,"unit":unit,
                "lower_limit":lower,"upper_limit":upper,"expected_cursor":cursor,"idempotency_key":format!("{id}-{item}")});
            let finish = |id:&str,cursor| json!({"inspection_id":id,"expected_cursor":cursor,"idempotency_key":format!("finish-{id}")});
            assert_eq!(invoke("create_inspection_plan_revision",create_plan(1,6,5,0)).await["code"],"INVALID_ARGUMENT");
            assert_eq!(invoke("create_inspection_plan_revision",create_plan(1,4,5,0)).await["plan"]["status"],"draft");
            assert_eq!(invoke("create_inspection",create("old","DIM@1",1)).await["code"],"PLAN_REVISION_NOT_ACTIVE");
            let active=invoke("activate_inspection_plan_revision",activate(1,1)).await;
            assert_eq!(active["plan"]["status"],"active","{active}");
            assert_eq!(invoke("activate_inspection_plan_revision",activate(1,1)).await,active);
            assert_eq!(invoke("create_inspection",create("old","DIM@1",2)).await["cursor"],3);
            assert_eq!(invoke("create_inspection_plan_revision",create_plan(2,6,7,3)).await["cursor"],4);
            assert_eq!(invoke("activate_inspection_plan_revision",activate(2,4)).await["cursor"],5);
            let mut obsolete=activate(1,5);
            obsolete["idempotency_key"]=json!("reactivate-old");
            assert_eq!(invoke("activate_inspection_plan_revision",obsolete).await["code"],"PLAN_REVISION_NOT_ACTIVATABLE");
            assert_eq!(invoke("create_inspection",create("new","DIM@1",5)).await["code"],"PLAN_REVISION_NOT_ACTIVE");
            assert_eq!(invoke("create_inspection",create("new","DIM@2",5)).await["cursor"],6);

            assert_eq!(invoke("record_measurement",measure("old","width",6,"mm",6,7,6)).await["code"],"MEASUREMENT_SPECIFICATION_MISMATCH");
            assert_eq!(invoke("record_measurement",measure("old","width",6,"cm",4,5,6)).await["field"],"unit");
            assert_eq!(invoke("record_measurement",measure("old","length",6,"mm",4,5,6)).await["code"],"CHARACTERISTIC_NOT_IN_PLAN");
            assert_eq!(invoke("record_measurement",measure("old","width",6,"mm",4,5,6)).await["measurement"]["passed"],false);
            assert_eq!(invoke("complete_inspection",finish("old",7)).await["code"],"PLAN_MEASUREMENTS_INCOMPLETE");
            assert_eq!(invoke("record_measurement",measure("old","height",1,"mm",1,2,7)).await["measurement"]["passed"],true);
            assert_eq!(invoke("complete_inspection",finish("old",8)).await["inspection"]["status"],"failed");
            let (peer,_child)=connect_mode(&db,"0",true).await;
            let old=business(&peer,run,"get_inspection",json!({"inspection_id":"old"})).await;
            assert_eq!(old["inspection_plan"]["status"],"superseded");
            assert_eq!(old["inspection_plan"]["characteristics"]["width"]["upper_limit"].as_f64(),Some(5.0));
            let new=invoke("get_inspection",json!({"inspection_id":"new"})).await;
            assert_eq!(new["inspection_plan"]["characteristics"]["width"]["upper_limit"].as_f64(),Some(7.0));
            let mut retry=create("retest","DIM@1",9);
            retry["reinspection_of"]=json!("new");
            assert_eq!(invoke("create_inspection",retry.clone()).await["code"],"REINSPECTION_NOT_ACCEPTABLE");
            retry["reinspection_of"]=json!("old");
            assert_eq!(invoke("create_inspection",retry).await["cursor"],10);
            let other=open().await;
            assert_eq!(business(&client,&other["run_id"],"create_inspection",create("foreign","DIM@2",0)).await["code"],"PLAN_REVISION_NOT_FOUND");
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p9a_02_item_revision_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("master.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        for (scenario, version) in [("cnc", 20), ("assembly", 19)] {
            let open = async || call(&client,"open_scenario",json!({"scenario_id":scenario,"version":version})).await;
            let opened = open().await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str,args| business(&client,run,name,args).await;
            let create=|item:&str,unit:&str,cursor| json!({"item_id":item,"base_unit":unit,
                "expected_cursor":cursor,"idempotency_key":format!("item-{item}")});
            let draft=|number,name:&str,cursor| json!({"item_id":"FRAME","revision":number,"name":name,
                "expected_cursor":cursor,"idempotency_key":format!("draft-{number}")});
            let activate=|number,cursor| json!({"revision_type":"item","item_id":"FRAME","revision":number,
                "expected_cursor":cursor,"idempotency_key":format!("activate-{number}")});
            let query=|number| json!({"item_id":"FRAME","revision":number});
            let search=call(&client,"search_tools",json!({"run_id":run,"query":"item","domain":"inventory"})).await;
            for name in ["list_items","get_item","create_item","create_item_revision","get_item_revision","activate_revision"] {
                assert!(search["tools"].as_array().unwrap().iter().any(|tool|tool["name"]==name),"{search}");
            }
            assert_eq!(invoke("create_item",create("FRAME","invalid",0)).await["field"],"base_unit");
            assert_eq!(invoke("create_item_revision",draft(1,"Frame A",0)).await["code"],"ITEM_NOT_FOUND");
            assert_eq!(invoke("create_item",create("FRAME","each",0)).await["cursor"],1);
            assert_eq!(invoke("create_item",create("WIRE","m",1)).await["cursor"],2);
            let listed=invoke("list_items",json!({"limit":1})).await;
            assert_eq!(listed["items"][0]["item_id"],"FRAME");
            assert_eq!(listed["next_after_item_id"],"FRAME");
            let next=invoke("list_items",json!({"limit":1,"after_item_id":listed["next_after_item_id"]})).await;
            assert_eq!(next["items"][0]["base_unit"],"m");
            assert!(next["next_after_item_id"].is_null());
            assert_eq!(invoke("create_item_revision",draft(0,"invalid",2)).await["field"],"revision");
            assert_eq!(invoke("create_item_revision",draft(1,"Frame A",2)).await["item_revision"]["status"],"draft");
            let first_args=activate(1,3);
            let first=invoke("activate_revision",first_args.clone()).await;
            assert_eq!(first["item_revision"]["status"],"active","{first}");
            assert_eq!(invoke("create_item_revision",draft(2,"Frame B",3)).await["code"],"VERSION_CONFLICT");
            assert_eq!(invoke("create_item_revision",draft(2,"Frame B",4)).await["cursor"],5);
            assert_eq!(invoke("activate_revision",activate(2,5)).await["item_revision"]["status"],"active");
            assert_eq!(invoke("activate_revision",first_args).await,first);
            let mut stale=activate(1,6);
            stale["idempotency_key"]=json!("superseded");
            assert_eq!(invoke("activate_revision",stale).await["code"],"ITEM_REVISION_NOT_ACTIVATABLE");
            let (peer,_child)=connect_mode(&db,"0",true).await;
            let old=business(&peer,run,"get_item_revision",query(1)).await;
            assert_eq!(old["item_revision"],json!({"item_id":"FRAME","revision":1,"name":"Frame A","status":"superseded"}));
            let current=business(&peer,run,"get_item",json!({"item_id":"FRAME"})).await;
            assert_eq!(current["item"],json!({"item_id":"FRAME","base_unit":"each","active_revision":2}));
            let isolated=open().await;
            assert_eq!(business(&peer,&isolated["run_id"],"get_item_revision",query(2)).await["code"],"ITEM_REVISION_NOT_FOUND");
            assert_eq!(call(&client,"get_run",json!({"run_id":run})).await["cursor"],6);
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p9a_04_bom_revision_graph() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("master.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        for (scenario,version) in [("cnc",21),("assembly",20)] {
            let open=async || call(&client,"open_scenario",json!({"scenario_id":scenario,"version":version})).await;
            let opened=open().await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str,args| business(&client,run,name,args).await;
            let write=async |name:&str,key:&str,mut args:Value| {
                args["expected_cursor"]=call(&client,"get_run",json!({"run_id":run})).await["cursor"].clone();
                args["idempotency_key"]=json!(key);
                invoke(name,args).await
            };
            let reference=|item:&str,revision| json!({"item_id":item,"revision":revision});
            let activate=|kind:&str,item:&str,revision| json!({"revision_type":kind,"item_id":item,"revision":revision});
            let set=|item:&str,revision,components| json!({"item_id":item,"revision":revision,"components":components});
            let component=|revision,unit:&str,expand_bom| json!({"revision":revision,"quantity":2,"unit":unit,"expand_bom":expand_bom});
            let search=call(&client,"search_tools",json!({"run_id":run,"query":"bom"})).await;
            assert!(search["tools"].as_array().unwrap().iter().any(|tool| tool["name"]=="create_bom_revision"));
            for (id,unit) in [("ROOT","each"),("SUB","each"),("WIRE","m")] {
                assert_eq!(write("create_item",&format!("item-{id}"),json!({"item_id":id,"base_unit":unit})).await["ok"],true);
                assert_eq!(write("create_item_revision",&format!("revision-{id}"),json!({"item_id":id,"revision":1,"name":id})).await["ok"],true);
            }
            for id in ["ROOT","SUB"] {
                assert_eq!(write("create_bom_revision",&format!("bom-{id}"),reference(id,1)).await["bom_revision"]["status"],"draft");
            }
            assert_eq!(write("set_bom_components","self-cycle",set("ROOT",1,json!({"ROOT":component(1,"each",false)}))).await["code"],"BOM_CYCLE");
            assert_eq!(write("set_bom_components","wrong-unit",set("ROOT",1,json!({"WIRE":component(1,"kg",false)}))).await["code"],"COMPONENT_UNIT_MISMATCH");
            assert_eq!(write("set_bom_components","missing",set("ROOT",1,json!({"WIRE":component(2,"m",false)}))).await["code"],"COMPONENT_REVISION_NOT_FOUND");
            assert_eq!(write("set_bom_components","root-sub",set("ROOT",1,json!({"SUB":component(1,"each",true)}))).await["ok"],true);
            assert_eq!(write("set_bom_components","cycle",set("SUB",1,json!({"ROOT":component(1,"each",true)}))).await["code"],"BOM_CYCLE");
            assert_eq!(invoke("get_bom_revision",reference("SUB",1)).await["bom_revision"]["components"],json!({}));
            assert_eq!(write("set_bom_components","sub-wire",set("SUB",1,json!({"WIRE":component(1,"m",false)}))).await["ok"],true);
            assert_eq!(invoke("validate_revision",activate("bom","ROOT",1)).await["code"],"BOM_DEPENDENCY_NOT_PUBLISHED");
            for id in ["ROOT","SUB","WIRE"] {
                assert_eq!(write("activate_revision",&format!("publish-item-{id}"),activate("item",id,1)).await["ok"],true);
            }
            assert_eq!(write("activate_revision","early-root",activate("bom","ROOT",1)).await["code"],"BOM_DEPENDENCY_NOT_PUBLISHED");
            assert_eq!(write("activate_revision","publish-sub",activate("bom","SUB",1)).await["bom_revision"]["status"],"active");
            assert_eq!(invoke("validate_revision",activate("bom","ROOT",1)).await["valid"],true);
            let mut publish=activate("bom","ROOT",1);
            publish["expected_cursor"]=call(&client,"get_run",json!({"run_id":run})).await["cursor"].clone();
            publish["idempotency_key"]=json!("publish-root");
            let first=invoke("activate_revision",publish.clone()).await;
            assert_eq!(first["bom_revision"]["status"],"active","{first}");
            assert_eq!(write("set_bom_components","immutable",set("ROOT",1,json!({}))).await["code"],"BOM_REVISION_IMMUTABLE");
            assert_eq!(write("create_item_revision","item-root2",json!({"item_id":"ROOT","revision":2,"name":"Root B"})).await["ok"],true);
            assert_eq!(write("activate_revision","publish-item-root2",activate("item","ROOT",2)).await["ok"],true);
            assert_eq!(write("create_bom_revision","bom-root2",reference("ROOT",2)).await["ok"],true);
            assert_eq!(write("set_bom_components","root2-wire",set("ROOT",2,json!({"WIRE":component(1,"m",false)}))).await["ok"],true);
            assert_eq!(write("activate_revision","publish-root2",activate("bom","ROOT",2)).await["bom_revision"]["status"],"active");
            assert_eq!(invoke("activate_revision",publish).await,first);
            let (peer,_child)=connect_mode(&db,"0",true).await;
            let old=business(&peer,run,"get_bom_revision",reference("ROOT",1)).await;
            assert_eq!(old["bom_revision"]["status"],"superseded");
            assert_eq!(old["bom_revision"]["components"]["SUB"]["revision"],1);
            assert_eq!(old["bom_revision"]["components"]["SUB"]["expand_bom"],true);
            let isolated=open().await;
            assert_eq!(business(&peer,&isolated["run_id"],"get_bom_revision",reference("ROOT",1)).await["code"],"BOM_REVISION_NOT_FOUND");
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p9a_06_route_revision_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("master.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        for (scenario,version) in [("cnc",22),("assembly",21)] {
            let open=async || call(&client,"open_scenario",json!({"scenario_id":scenario,"version":version})).await;
            let opened=open().await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str,args| business(&client,run,name,args).await;
            let cursor=async || call(&client,"get_run",json!({"run_id":run})).await["cursor"].clone();
            let write=async |name:&str,key:&str,mut args:Value| {
                args["expected_cursor"]=cursor().await;
                args["idempotency_key"]=json!(key);
                invoke(name,args).await
            };
            let reference=|revision| json!({"item_id":"FRAME","revision":revision});
            let activate=|kind:&str,revision| json!({"revision_type":kind,"item_id":"FRAME","revision":revision});
            let definition=|revision,operations| json!({"item_id":"FRAME","revision":revision,"operations":operations});
            let op=|center:&str,predecessors:&[&str]| json!({"work_center":center,"predecessors":predecessors});
            let search=call(&client,"search_tools",json!({"run_id":run,"query":"route"})).await;
            for name in ["create_route_revision","get_route_revision","set_route_operations"] {
                assert!(search["tools"].as_array().unwrap().iter().any(|tool|tool["name"]==name),"{search}");
            }
            let spec=call(&client,"describe_tool",json!({"run_id":run,"name":"activate_revision"})).await;
            assert_eq!(spec["tool"]["input"]["properties"]["revision_type"]["enum"],json!(["item","bom","route"]));
            assert_eq!(write("create_route_revision","route1",reference(1)).await["code"],"ITEM_REVISION_NOT_FOUND");
            assert_eq!(write("create_item","item",json!({"item_id":"FRAME","base_unit":"each"})).await["ok"],true);
            assert_eq!(write("create_item_revision","item1",json!({"item_id":"FRAME","revision":1,"name":"Frame A"})).await["ok"],true);
            assert_eq!(write("create_route_revision","route1",reference(1)).await["route_revision"]["status"],"draft");
            assert_eq!(invoke("validate_revision",activate("route",1)).await["code"],"ROUTE_OPERATIONS_REQUIRED");
            let before=cursor().await;
            assert_eq!(write("set_route_operations","missing",definition(1,json!({"test":op("QA-01",&["missing"])}))).await["code"],"ROUTE_PREDECESSOR_NOT_FOUND");
            assert_eq!(write("set_route_operations","cycle",definition(1,json!({"build":op("CELL-A",&["test"]),"test":op("QA-01",&["build"])}))).await["code"],"ROUTE_CYCLE");
            assert_eq!(cursor().await,before);
            assert_eq!(invoke("get_route_revision",reference(1)).await["route_revision"]["operations"],json!({}));
            let configured=write("set_route_operations","configure1",definition(1,json!({
                "test":op("QA-01",&["build","label"]),"label":op("CELL-B",&[]),"build":op("CELL-A",&[])
            }))).await;
            assert_eq!(configured["route_revision"]["execution_order"],json!(["build","label","test"]),"{configured}");
            assert_eq!(invoke("validate_revision",activate("route",1)).await["code"],"ROUTE_ITEM_REVISION_NOT_PUBLISHED");
            assert_eq!(write("activate_revision","publish-item1",activate("item",1)).await["ok"],true);
            assert_eq!(invoke("validate_revision",activate("route",1)).await["valid"],true);
            let mut publish=activate("route",1);
            publish["expected_cursor"]=cursor().await;
            publish["idempotency_key"]=json!("publish-route1");
            let first=invoke("activate_revision",publish.clone()).await;
            assert_eq!(first["route_revision"]["status"],"active","{first}");
            assert_eq!(write("set_route_operations","immutable",definition(1,json!({}))).await["code"],"ROUTE_REVISION_IMMUTABLE");
            assert_eq!(write("create_item_revision","item2",json!({"item_id":"FRAME","revision":2,"name":"Frame B"})).await["ok"],true);
            assert_eq!(write("activate_revision","publish-item2",activate("item",2)).await["ok"],true);
            assert_eq!(write("create_route_revision","route2",reference(2)).await["ok"],true);
            assert_eq!(write("set_route_operations","configure2",definition(2,json!({"assemble":op("CELL-C",&[])}))).await["ok"],true);
            assert_eq!(write("activate_revision","publish-route2",activate("route",2)).await["route_revision"]["status"],"active");
            assert_eq!(invoke("activate_revision",publish).await,first);
            let (peer,_child)=connect_mode(&db,"0",true).await;
            let old=business(&peer,run,"get_route_revision",reference(1)).await;
            assert_eq!(old["route_revision"]["status"],"superseded");
            assert_eq!(old["route_revision"]["operations"],first["route_revision"]["operations"]);
            let isolated=open().await;
            assert_eq!(business(&peer,&isolated["run_id"],"get_route_revision",reference(1)).await["code"],"ROUTE_REVISION_NOT_FOUND");
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p9a_08_release_snapshot_and_change_impact() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("master.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        for (scenario,version,order,other) in [("cnc",23,"WO-1","WO-2"),("assembly",22,"WO-E","WO-L")] {
            let opened=call(&client,"open_scenario",json!({"scenario_id":scenario,"version":version})).await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str,args| business(&client,run,name,args).await;
            let cursor=async || call(&client,"get_run",json!({"run_id":run})).await["cursor"].clone();
            let write=async |name:&str,key:&str,mut args:Value| {
                args["expected_cursor"]=cursor().await;
                args["idempotency_key"]=json!(key);
                let result=invoke(name,args).await;
                assert_eq!(result["ok"],true,"{name}: {result}");
                result
            };
            let input=|id:&str| json!({"work_order":id,"definition_selection":{"item_id":"FRAME","inspection_plan_id":"QA"}});
            let ready=|id:&str| invoke("get_release_readiness",input(id));
            assert_eq!(ready(order).await["code"],"MASTER_DEFINITION_NOT_ACTIVE");
            for item in ["FRAME","WIRE"] {
                write("create_item",&format!("item-{item}"),json!({"item_id":item,"base_unit":"each"})).await;
            }
            write("create_item_revision","wire-v1",json!({"item_id":"WIRE","revision":1,"name":"Wire"})).await;
            write("activate_revision","wire-active",json!({"revision_type":"item","item_id":"WIRE","revision":1})).await;
            let configure=async |n| {
                for (name,args) in [
                    ("create_item_revision",json!({"item_id":"FRAME","revision":n,"name":format!("Frame {n}")})),
                    ("activate_revision",json!({"revision_type":"item","item_id":"FRAME","revision":n})),
                    ("create_bom_revision",json!({"item_id":"FRAME","revision":n})),
                    ("set_bom_components",json!({"item_id":"FRAME","revision":n,"components":{"WIRE":{"revision":1,"quantity":n,"unit":"each"}}})),
                    ("activate_revision",json!({"revision_type":"bom","item_id":"FRAME","revision":n})),
                    ("create_route_revision",json!({"item_id":"FRAME","revision":n})),
                    ("set_route_operations",json!({"item_id":"FRAME","revision":n,"operations":{"build":{"work_center":format!("CELL-{n}"),"predecessors":[]}}})),
                    ("activate_revision",json!({"revision_type":"route","item_id":"FRAME","revision":n})),
                    ("create_inspection_plan_revision",json!({"plan_id":"QA","revision":n,"characteristics":{"width":{"unit":"mm","lower_limit":1,"upper_limit":n+1}}})),
                    ("activate_inspection_plan_revision",json!({"plan_id":"QA","revision":n})),
                ].into_iter().enumerate().map(|(i,(name,args))| (format!("{n}-{i}"),(name,args))) {
                    let (tool,body)=args;
                    write(tool,&name,body).await;
                }
            };
            configure(1).await;
            let before=ready(order).await;
            assert_eq!(before["ready"],true,"{before}");
            let stale=ready(other).await;
            let mut release=input(order);
            release["snapshot_fingerprint"]=before["definition_snapshot"]["fingerprint"].clone();
            release["expected_cursor"]=before["cursor"].clone();
            release["idempotency_key"]=json!("release-first");
            let first=invoke("release_work_order",release.clone()).await;
            assert_eq!(first["status"],"released","{first}");
            assert_eq!(first["definition_snapshot"],before["definition_snapshot"]);
            configure(2).await;
            let mut obsolete=input(other);
            obsolete["snapshot_fingerprint"]=stale["definition_snapshot"]["fingerprint"].clone();
            obsolete["expected_cursor"]=cursor().await;
            obsolete["idempotency_key"]=json!("release-other");
            let unchanged=cursor().await;
            assert_eq!(invoke("release_work_order",obsolete.clone()).await["code"],"SNAPSHOT_CHANGED");
            assert_eq!(cursor().await,unchanged);
            assert_eq!(invoke("get_order_snapshot",json!({"work_order":other})).await["code"],"ORDER_SNAPSHOT_NOT_FOUND");
            let fresh=ready(other).await;
            assert_eq!(fresh["ready"],true,"{fresh}");
            obsolete["snapshot_fingerprint"]=fresh["definition_snapshot"]["fingerprint"].clone();
            assert_eq!(invoke("release_work_order",obsolete).await["definition_snapshot"]["item"]["revision"],2);
            assert_eq!(invoke("release_work_order",release).await,first);
            let (peer,_child)=connect_mode(&db,"0",true).await;
            let frozen=business(&peer,run,"get_order_snapshot",json!({"work_order":order})).await;
            assert_eq!(frozen["snapshot"],first["definition_snapshot"]);
            let impact=business(&peer,run,"assess_engineering_change",json!({"work_order":order})).await;
            assert_eq!(impact["changed"],json!(["item","bom","route","inspection_plan"]),"{impact}");
            let mut inspection=json!({"inspection_id":"QC-OLD","work_order":order,"specification_version":"QA@2",
                "expected_cursor":cursor().await,"idempotency_key":"inspection"});
            assert_eq!(invoke("create_inspection",inspection.clone()).await["code"],"INSPECTION_PLAN_SNAPSHOT_MISMATCH");
            inspection["specification_version"]=json!("QA@1");
            assert_eq!(invoke("create_inspection",inspection).await["inspection"]["specification_version"],"QA@1");
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p9b_01_receipt_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("inventory.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    timeout(WAIT, async {
        for (scenario,version) in [("cnc",24),("assembly",23)] {
            let open=async || call(&client,"open_scenario",json!({"scenario_id":scenario,"version":version})).await;
            let opened=open().await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str,args| business(&client,run,name,args).await;
            let write=async |name:&str,key:&str,mut args:Value| {
                args["expected_cursor"]=call(&client,"get_run",json!({"run_id":run})).await["cursor"].clone();
                args["idempotency_key"]=json!(key);
                invoke(name,args).await
            };
            let draft=|id:&str,amount:&str,pending| json!({"receipt_id":id,"item_id":"WIRE",
                "revision":1,"quantity":amount,"requires_inspection":pending});
            let track=|id:&str,lot:&str,location:&str|json!({"receipt_id":id,"lot_id":lot,
                "supplier_lot":"SUP-1","location":location});
            assert_eq!(invoke("list_stock_locations",json!({})).await["locations"],json!(["RAW-01","RAW-02"]));
            assert_eq!(write("create_item","item",json!({"item_id":"WIRE","base_unit":"m"})).await["ok"],true);
            assert_eq!(write("create_item_revision","revision",json!({"item_id":"WIRE","revision":1,"name":"Wire"})).await["ok"],true);
            assert_eq!(write("create_receipt","receipt-a",draft("RC-A","1.25",true)).await["code"],"ITEM_REVISION_NOT_PUBLISHED");
            assert_eq!(write("activate_revision","active",json!({"revision_type":"item","item_id":"WIRE","revision":1})).await["ok"],true);
            assert_eq!(write("create_receipt","precision",draft("RC-A","1.0001",true)).await["field"],"quantity");
            assert_eq!(write("create_receipt","receipt-a",draft("RC-A","1.250",true)).await["receipt"]["quantity"],"1.25");
            assert_eq!(write("create_receipt","receipt-b",draft("RC-B","2.005",false)).await["receipt"]["status"],"draft");
            assert_eq!(write("post_receipt","post-a",json!({"receipt_id":"RC-A"})).await["code"],"RECEIPT_TRACKING_REQUIRED");
            assert_eq!(write("assign_receipt_tracking","track-a",track("RC-A","LOT-A","RAW-03")).await["code"],"LOCATION_NOT_FOUND");
            assert_eq!(write("assign_receipt_tracking","track-a",track("RC-A","LOT-A","RAW-01")).await["receipt"]["status"],"tracked");
            assert_eq!(write("assign_receipt_tracking","duplicate",track("RC-B","LOT-A","RAW-02")).await["code"],"LOT_ALREADY_ASSIGNED");
            assert_eq!(write("assign_receipt_tracking","track-b",track("RC-B","LOT-B","RAW-02")).await["ok"],true);
            assert_eq!(invoke("get_lot",json!({"lot_id":"LOT-A"})).await["code"],"LOT_NOT_FOUND");
            assert_eq!(invoke("get_stock_balance",json!({"item_id":"WIRE"})).await["balance"]["on_hand"],"0");
            let args=json!({"receipt_id":"RC-A","expected_cursor":7,"idempotency_key":"post-a"});
            let (peer,_child)=connect_mode(&db,"0",true).await;
            let (first,replay)=tokio::join!(invoke("post_receipt",args.clone()),business(&peer,run,"post_receipt",args));
            assert_eq!(first,replay);
            assert_eq!(first["receipt"]["availability"],"pending_inspection","{first}");
            assert_eq!(write("post_receipt","post-a-again",json!({"receipt_id":"RC-A"})).await["code"],"RECEIPT_ALREADY_POSTED");
            assert_eq!(write("post_receipt","post-b",json!({"receipt_id":"RC-B"})).await["receipt"]["availability"],"available");
            let stock=business(&peer,run,"get_stock_balance",json!({"item_id":"WIRE","revision":1})).await;
            assert_eq!(stock["balance"]["on_hand"],"3.255");
            assert_eq!(stock["balance"]["available"],"2.005");
            assert_eq!(stock["balance"]["pending_inspection"],"1.25");
            let location=invoke("get_stock_balance",json!({"item_id":"WIRE","location":"RAW-01"})).await;
            assert_eq!(location["balance"]["available"],"0");
            let page=invoke("list_lots",json!({"limit":1})).await;
            assert_eq!(page["lots"][0]["lot_id"],"LOT-A");
            let next=invoke("list_lots",json!({"limit":1,"after_receipt_id":page["next_after_receipt_id"]})).await;
            assert_eq!(next["lots"][0]["lot_id"],"LOT-B");
            assert!(next["next_after_receipt_id"].is_null());
            let movements=invoke("list_inventory_movements",json!({"item_id":"WIRE"})).await;
            assert_eq!(movements["movements"].as_array().unwrap().len(),2);
            assert_eq!(movements["movements"][0],first["receipt"]);
            let isolated=open().await;
            assert_eq!(business(&peer,&isolated["run_id"],"get_lot",json!({"lot_id":"LOT-A"})).await["code"],"LOT_NOT_FOUND");
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p9b_02_receipt_quality_and_requirements() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _child) = connect_mode(&dir.path().join("inventory.sqlite3"), "0", true).await;
    timeout(Duration::from_secs(30), async {
        for (scenario,version,order,needed) in [("cnc",25,"WO-1","25"),("assembly",24,"WO-E","2.5")] {
            let opened=call(&client,"open_scenario",json!({"scenario_id":scenario,"version":version})).await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str,args| business(&client,run,name,args).await;
            let cursor=async || call(&client,"get_run",json!({"run_id":run})).await["cursor"].clone();
            let write=async |name:&str,key:&str,mut args:Value| {
                args["expected_cursor"]=cursor().await;
                args["idempotency_key"]=json!(key);
                invoke(name,args).await
            };
            let apply=async |name:&str,key:&str,args| {
                let result=write(name,key,args).await;
                assert_eq!(result["ok"],true,"{name}: {result}");
                result
            };
            let item=|id:&str,revision| json!({"item_id":id,"revision":revision});
            let demand=|| invoke("get_material_requirements",json!({"work_order":order}));
            assert_eq!(demand().await["code"],"ORDER_SNAPSHOT_NOT_FOUND");
            for (id,unit) in [("FRAME","each"),("SUB","each"),("WIRE","m")] {
                apply("create_item",id,json!({"item_id":id,"base_unit":unit})).await;
                apply("create_item_revision",&format!("draft-{id}"),json!({"item_id":id,"revision":1,"name":id})).await;
                apply("activate_revision",&format!("active-{id}"),json!({"revision_type":"item","item_id":id,"revision":1})).await;
            }
            for (parent,components) in [
                ("SUB",json!({"WIRE":{"revision":1,"quantity":1.25,"unit":"m"}})),
                ("FRAME",json!({"SUB":{"revision":1,"quantity":2,"unit":"each","expand_bom":true}})),
            ] {
                apply("create_bom_revision",&format!("bom-{parent}"),item(parent,1)).await;
                apply("set_bom_components",&format!("components-{parent}"),json!({"item_id":parent,"revision":1,"components":components})).await;
                apply("activate_revision",&format!("active-bom-{parent}"),json!({"revision_type":"bom","item_id":parent,"revision":1})).await;
            }
            apply("create_route_revision","route",item("FRAME",1)).await;
            apply("set_route_operations","operations",json!({"item_id":"FRAME","revision":1,"operations":{"build":{"work_center":"CELL-A","predecessors":[]}}})).await;
            apply("activate_revision","active-route",json!({"revision_type":"route","item_id":"FRAME","revision":1})).await;
            apply("create_inspection_plan_revision","plan",json!({"plan_id":"QA","revision":1,"characteristics":{"width":{"unit":"mm","lower_limit":1,"upper_limit":2}}})).await;
            apply("activate_inspection_plan_revision","active-plan",json!({"plan_id":"QA","revision":1})).await;
            let mut release=json!({"work_order":order,"definition_selection":{"item_id":"FRAME","inspection_plan_id":"QA"}});
            let readiness=invoke("get_release_readiness",release.clone()).await;
            assert_eq!(readiness["ready"],true,"{readiness}");
            release["snapshot_fingerprint"]=readiness["definition_snapshot"]["fingerprint"].clone();
            apply("release_work_order","release",release).await;
            apply("create_item_revision","wire2",json!({"item_id":"WIRE","revision":2,"name":"Wire next"})).await;
            apply("activate_revision","active-wire2",json!({"revision_type":"item","item_id":"WIRE","revision":2})).await;
            for (id,lot,revision,quantity,inspect) in [("RC-1","LOT-1",1,"2",true),("RC-2","LOT-2",2,"9",false)] {
                apply("create_receipt",id,json!({"receipt_id":id,"item_id":"WIRE","revision":revision,
                    "quantity":quantity,"requires_inspection":inspect})).await;
                apply("assign_receipt_tracking",lot,json!({"receipt_id":id,"lot_id":lot,"supplier_lot":"SUP-1","location":"RAW-01"})).await;
                apply("post_receipt",&format!("post-{id}"),json!({"receipt_id":id})).await;
            }
            let available=||invoke("get_material_availability",item("WIRE",1));
            assert_eq!(available().await["balance"]["available"],"0");
            assert_eq!(demand().await["requirements"][0]["required"],needed);
            assert_eq!(demand().await["requirements"][0]["shortage"],needed);
            let inspect=async |id:&str,value| {
                apply("create_inspection",id,json!({"work_order":order,"inspection_id":id,"specification_version":"QA@1","receipt_id":"RC-1"})).await;
                apply("record_measurement",&format!("measure-{id}"),json!({"inspection_id":id,"characteristic":"width",
                    "value":value,"unit":"mm","lower_limit":1,"upper_limit":2})).await;
                apply("complete_inspection",&format!("finish-{id}"),json!({"inspection_id":id})).await
            };
            let disposition=|id:&str|json!({"receipt_id":"RC-1","inspection_id":id});
            assert_eq!(inspect("bad",3).await["inspection"]["status"],"failed");
            assert_eq!(write("release_receipt_inspection","release-stock",disposition("bad")).await["code"],"RECEIPT_INSPECTION_NOT_PASSED");
            assert_eq!(inspect("good",2).await["inspection"]["status"],"passed");
            apply("create_receipt","rc3",json!({"receipt_id":"RC-3","item_id":"WIRE","revision":2,
                "quantity":"1","requires_inspection":true})).await;
            apply("assign_receipt_tracking","track-rc3",json!({"receipt_id":"RC-3","lot_id":"LOT-3",
                "supplier_lot":"SUP-2","location":"RAW-02"})).await;
            apply("post_receipt","post-rc3",json!({"receipt_id":"RC-3"})).await;
            assert_eq!(write("release_receipt_inspection","foreign-evidence",json!({"receipt_id":"RC-3","inspection_id":"good"})).await["code"],"RECEIPT_INSPECTION_NOT_PASSED");
            let mut approved=disposition("good");
            approved["expected_cursor"]=cursor().await;
            approved["idempotency_key"]=json!("release-stock");
            let first=invoke("release_receipt_inspection",approved.clone()).await;
            assert_eq!(first["receipt"]["availability"],"available","{first}");
            assert_eq!(first["receipt"]["released_inspection_id"],"good");
            assert_eq!(invoke("release_receipt_inspection",approved).await,first);
            assert_eq!(available().await["balance"]["on_hand"],"2");
            let requirements=demand().await;
            assert_eq!(requirements["requirements"][0],json!({"item_id":"WIRE","revision":1,"unit":"m",
                "required":needed,"available":"2","shortage":if scenario=="cnc" {"23"} else {"0.5"}}),"{requirements}");
            assert_eq!(invoke("get_lot",json!({"lot_id":"LOT-1"})).await["lot"]["released_cursor"],first["cursor"]);
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p9c_01_reservation_contention_and_picking() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("inventory.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    timeout(Duration::from_secs(30), async {
        for (scenario,version,orders) in [("cnc",26,["WO-1","WO-2"]),("assembly",25,["WO-E","WO-L"])] {
            let opened=call(&client,"open_scenario",json!({"scenario_id":scenario,"version":version})).await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str,args| business(&client,run,name,args).await;
            let cursor=async || call(&client,"get_run",json!({"run_id":run})).await["cursor"].clone();
            let write=async |name:&str,key:&str,mut args:Value| {
                args["expected_cursor"]=cursor().await;
                args["idempotency_key"]=json!(key);
                invoke(name,args).await
            };
            let apply=async |name:&str,key:&str,args| {
                let result=write(name,key,args).await;
                assert_eq!(result["ok"],true,"{name}: {result}");
                result
            };
            for (name,args) in [
                ("create_item",json!({"item_id":"FRAME","base_unit":"each"})),
                ("create_item",json!({"item_id":"WIRE","base_unit":"m"})),
                ("create_item_revision",json!({"item_id":"FRAME","revision":1,"name":"Frame"})),
                ("create_item_revision",json!({"item_id":"WIRE","revision":1,"name":"Wire"})),
                ("activate_revision",json!({"revision_type":"item","item_id":"FRAME","revision":1})),
                ("activate_revision",json!({"revision_type":"item","item_id":"WIRE","revision":1})),
                ("create_bom_revision",json!({"item_id":"FRAME","revision":1})),
                ("set_bom_components",json!({"item_id":"FRAME","revision":1,"components":{"WIRE":{"revision":1,"quantity":2,"unit":"m"}}})),
                ("activate_revision",json!({"revision_type":"bom","item_id":"FRAME","revision":1})),
                ("create_route_revision",json!({"item_id":"FRAME","revision":1})),
                ("set_route_operations",json!({"item_id":"FRAME","revision":1,"operations":{"build":{"work_center":"CELL-A","predecessors":[]}}})),
                ("activate_revision",json!({"revision_type":"route","item_id":"FRAME","revision":1})),
                ("create_inspection_plan_revision",json!({"plan_id":"QA","revision":1,"characteristics":{"width":{"unit":"mm","lower_limit":1,"upper_limit":2}}})),
                ("activate_inspection_plan_revision",json!({"plan_id":"QA","revision":1})),
            ].into_iter().enumerate().map(|(i,(name,args))| (format!("setup-{i}"),(name,args))) {
                apply(args.0,&name,args.1).await;
            }
            for order in orders {
                let mut args=json!({"work_order":order,"definition_selection":{"item_id":"FRAME","inspection_plan_id":"QA"}});
                let readiness=invoke("get_release_readiness",args.clone()).await;
                assert_eq!(readiness["ready"],true,"{readiness}");
                args["snapshot_fingerprint"]=readiness["definition_snapshot"]["fingerprint"].clone();
                apply("release_work_order",order,args).await;
            }
            apply("create_item_revision","wire2",json!({"item_id":"WIRE","revision":2,"name":"Wire next"})).await;
            apply("activate_revision","wire2-active",json!({"revision_type":"item","item_id":"WIRE","revision":2})).await;
            for (receipt,lot,revision,pending) in [("RC-A","LOT-A",1,false),("RC-B","LOT-B",1,true),("RC-C","LOT-C",2,false)] {
                apply("create_receipt",receipt,json!({"receipt_id":receipt,"item_id":"WIRE","revision":revision,"quantity":"2","requires_inspection":pending})).await;
                apply("assign_receipt_tracking",lot,json!({"receipt_id":receipt,"lot_id":lot,"location":"RAW-02","supplier_lot":"SUP-1"})).await;
                apply("post_receipt",&format!("post-{receipt}"),json!({"receipt_id":receipt})).await;
            }
            let reserve=|id:&str,order:&str,lot:&str,quantity:&str| json!({"reservation_id":id,"work_order":order,"lot_id":lot,"quantity":quantity});
            assert_eq!(write("reserve_material","pending",reserve("R-P",orders[0],"LOT-B","1")).await["code"],"LOT_PENDING_INSPECTION");
            assert_eq!(write("reserve_material","wrong-version",reserve("R-W",orders[0],"LOT-C","1")).await["code"],"MATERIAL_NOT_REQUIRED");
            let current=cursor().await;
            let mut a=reserve("R-A",orders[0],"LOT-A","1.5");
            let mut b=reserve("R-B",orders[1],"LOT-A","1.5");
            for (args,key) in [(&mut a,"a"),(&mut b,"b")] {
                args["expected_cursor"]=current.clone();
                args["idempotency_key"]=json!(key);
            }
            let (peer,_child)=connect_mode(&db,"0",true).await;
            let (first,second)=tokio::join!(invoke("reserve_material",a.clone()),business(&peer,run,"reserve_material",b.clone()));
            let (winner,winner_args,mut loser_args,loser)=if first["ok"]==true {
                (first,a,b,second)
            } else {(second,b,a,first)};
            assert_eq!(winner["reservation"]["status"],"reserved","{winner}");
            assert_eq!(loser["code"],"VERSION_CONFLICT","{loser}");
            loser_args["expected_cursor"]=cursor().await;
            assert_eq!(invoke("reserve_material",loser_args).await["code"],"LOT_AVAILABLE_QUANTITY_EXCEEDED");
            let id=&winner["reservation"]["reservation_id"];
            let stock=||invoke("get_stock_balance",json!({"item_id":"WIRE","revision":1,"location":"RAW-02"}));
            assert_eq!(stock().await["balance"]["reserved"],"1.5");
            assert_eq!(stock().await["balance"]["available"],"0.5");
            let requirement=invoke("get_material_requirements",json!({"work_order":winner["reservation"]["work_order"]})).await;
            assert_eq!(requirement["requirements"][0]["reserved"],"1.5");
            if scenario=="assembly" {
                let order=winner["reservation"]["work_order"].as_str().unwrap();
                assert_eq!(write("reserve_material","excess-demand",reserve("R-X",order,"LOT-A","0.6")).await["code"],"ORDER_REQUIREMENT_EXCEEDED");
            }
            assert_eq!(apply("pick_material","pick",json!({"reservation_id":id})).await["reservation"]["status"],"picked");
            assert_eq!(write("release_reservation","early-release",json!({"reservation_id":id})).await["code"],"RESERVATION_STATE_INVALID");
            assert_eq!(stock().await["balance"]["on_hand"],"4");
            apply("unpick_material","unpick",json!({"reservation_id":id})).await;
            apply("release_reservation","release",json!({"reservation_id":id})).await;
            assert_eq!(stock().await["balance"]["available"],"2");
            assert_eq!(invoke("reserve_material",winner_args).await,winner);
            let list=business(&peer,run,"list_reservations",json!({"lot_id":"LOT-A"})).await;
            assert_eq!(list["reservations"][0]["status"],"released");
        }
    }).await.unwrap();
}

#[tokio::test]
async fn p9c_02_issue_to_operation() {
    operation_issue_scenarios(&[
        ("cnc", 27, "WO-1", "machining", "MACH-01"),
        ("assembly", 26, "WO-E", "build", "CELL-A"),
    ])
    .await;
}

#[tokio::test]
async fn p9c_04_record_issued_report_inputs() {
    operation_issue_scenarios(&[("cnc", 28, "WO-1", "machining", "MACH-01")]).await;
}

#[tokio::test]
async fn p9c_06_return_unused_material() {
    operation_issue_scenarios(&[("cnc", 29, "WO-1", "machining", "MACH-01")]).await;
}

#[tokio::test]
async fn p9c_08_stock_transfer() {
    operation_issue_scenarios(&[("cnc", 30, "WO-1", "machining", "MACH-01")]).await;
}

#[tokio::test]
async fn p9c_10_stock_transfer_reversal() {
    operation_issue_scenarios(&[("cnc", 31, "WO-1", "machining", "MACH-01")]).await;
}

#[tokio::test]
async fn p9c_12_receipt_reversal() {
    operation_issue_scenarios(&[("cnc", 32, "WO-1", "machining", "MACH-01")]).await;
}

#[tokio::test]
async fn p9c_14_operation_material_reversals() {
    operation_issue_scenarios(&[("cnc", 33, "WO-1", "machining", "MACH-01")]).await;
}

#[tokio::test]
async fn p9c_16_assembly_material_reconciliation() {
    operation_issue_scenarios(&[("assembly", 27, "WO-E", "build", "CELL-A")]).await;
}

#[tokio::test]
async fn p9c_18_partial_lot_transfers() {
    operation_issue_scenarios(&[
        ("cnc", 34, "WO-1", "machining", "MACH-01"),
        ("assembly", 28, "WO-E", "build", "CELL-A"),
    ])
    .await;
}

async fn partial_lot_transfer(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    order: &str,
    operation: &str,
    center: &str,
) {
    let invoke = async |name: &str, args| business(client, run, name, args).await;
    let cursor = async || call(client, "get_run", json!({"run_id":run})).await["cursor"].clone();
    let write = async |name: &str, key: &str, mut args: Value| {
        args["idempotency_key"] = json!(key);
        args["expected_cursor"] = cursor().await;
        invoke(name, args).await
    };
    let apply = async |name: &str, key: &str, args| {
        let result = write(name, key, args).await;
        assert_eq!(result["ok"], true, "{name}: {result}");
        result
    };
    let stock = async |location: &str| {
        invoke(
            "get_stock_balance",
            json!({"item_id":"WIRE","location":location}),
        )
        .await["balance"]
            .clone()
    };
    let draft = json!({"transfer_id":"TR-1","lot_id":"LOT-1","child_lot_id":"LOT-2","quantity":"0.75",
        "source_location":"RAW-02","destination_location":"RAW-01","reason":"split stock"});
    let post = json!({"transfer_id":"TR-1"});
    let reversal = json!({"movement_type":"transfer","movement_id":"TR-1","reversal_id":"TR-2","reason":"correct location"});
    apply("create_stock_transfer", "draft-split", draft.clone()).await;
    assert_eq!(stock("RAW-02").await["on_hand"], "2");
    assert_eq!(
        invoke("get_lot", json!({"lot_id":"LOT-2"})).await["code"],
        "LOT_NOT_FOUND"
    );
    apply(
        "reserve_material",
        "reserve-parent",
        json!({"reservation_id":"RS-1","work_order":order,"lot_id":"LOT-1","quantity":"1.5"}),
    )
    .await;
    let before = cursor().await;
    assert_eq!(
        write("post_stock_transfer", "occupied", post.clone()).await["code"],
        "LOT_AVAILABLE_QUANTITY_EXCEEDED"
    );
    assert_eq!(cursor().await, before);
    apply(
        "release_reservation",
        "release-parent",
        json!({"reservation_id":"RS-1"}),
    )
    .await;
    apply(
        "reserve_material",
        "reserve-remainder",
        json!({"reservation_id":"RS-2","work_order":order,"lot_id":"LOT-1","quantity":"1"}),
    )
    .await;
    let moved = apply("post_stock_transfer", "posted-split", post).await;
    assert_eq!(moved["transfer"]["child_lot_id"], "LOT-2");
    assert_eq!(stock("RAW-02").await["on_hand"], "1.25");
    assert_eq!(stock("RAW-01").await["on_hand"], "0.75");
    assert_eq!(stock("RAW-02").await["reserved"], "1");
    let child = invoke("get_lot", json!({"lot_id":"LOT-2"})).await;
    assert_eq!(child["lot"]["parent_lot_id"], "LOT-1");
    assert_eq!(child["lot"]["receipt_id"], "RC-1");
    let page = invoke("list_lots", json!({"include_split_lots":true,"limit":1})).await;
    assert_eq!(page["next_after_lot_id"], "LOT-1");
    assert_eq!(
        invoke(
            "list_lots",
            json!({"include_split_lots":true,"after_lot_id":"LOT-1"})
        )
        .await["lots"],
        json!([child["lot"]])
    );
    assert_eq!(
        invoke(
            "list_inventory_movements",
            json!({"movement_type":"receipt"})
        )
        .await["movements"][0]["quantity"],
        "2"
    );
    apply(
        "reserve_material",
        "reserve-child",
        json!({"reservation_id":"RS-3","work_order":order,"lot_id":"LOT-2","quantity":"0.5"}),
    )
    .await;
    assert_eq!(
        write(
            "reverse_inventory_movement",
            "child-committed",
            reversal.clone()
        )
        .await["code"],
        "LOT_COMMITTED"
    );
    apply(
        "release_reservation",
        "release-child",
        json!({"reservation_id":"RS-3"}),
    )
    .await;
    let reversed = apply("reverse_inventory_movement", "reverse-split", reversal).await;
    assert_eq!(reversed["transfer"]["reverses_transfer_id"], "TR-1");
    assert_eq!(stock("RAW-02").await["on_hand"], "2");
    assert_eq!(stock("RAW-01").await["on_hand"], "0");
    assert_eq!(
        invoke("get_lot", json!({"lot_id":"LOT-2"})).await["code"],
        "LOT_NOT_FOUND"
    );
    let mut draft = draft;
    draft["transfer_id"] = json!("TR-3");
    draft["child_lot_id"] = json!("LOT-3");
    apply("create_stock_transfer", "draft-second", draft).await;
    apply(
        "post_stock_transfer",
        "post-second",
        json!({"transfer_id":"TR-3"}),
    )
    .await;
    apply(
        "reserve_material",
        "reserve-issued-child",
        json!({"reservation_id":"RS-4","work_order":order,"lot_id":"LOT-3","quantity":"0.5"}),
    )
    .await;
    apply(
        "pick_material",
        "pick-child",
        json!({"reservation_id":"RS-4"}),
    )
    .await;
    apply(
        "dispatch_operation",
        "dispatch-child",
        json!({"work_order":order,"operation":operation,"work_center":center}),
    )
    .await;
    let issue = apply(
        "issue_material_to_operation",
        "issue-child",
        json!({"reservation_id":"RS-4","operation":operation}),
    )
    .await;
    assert_eq!(issue["reservation"]["location"], "RAW-01");
    assert_eq!(stock("RAW-01").await["on_hand"], "0.25");
    assert_eq!(write("reverse_inventory_movement","dependent-child",json!({"movement_type":"transfer","movement_id":"TR-3","reversal_id":"TR-4","reason":"correct location"})).await["code"],"TRANSFER_HAS_DEPENDENCIES");
}

async fn assembly_material_reconciliation(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    order: &str,
    operation: &str,
    center: &str,
) {
    let invoke = async |name: &str, args| business(client, run, name, args).await;
    let cursor = async || call(client, "get_run", json!({"run_id":run})).await["cursor"].clone();
    let write = async |name: &str, key: &str, mut args: Value| {
        args["expected_cursor"] = cursor().await;
        args["idempotency_key"] = json!(key);
        invoke(name, args).await
    };
    let apply = async |name: &str, key: &str, args| {
        let result = write(name, key, args).await;
        assert_eq!(result["ok"], true, "{name}: {result}");
        result
    };
    let build = json!({"work_order":order,"operation":operation});
    let reservation = json!({"reservation_id":"RS-1"});
    apply(
        "dispatch_operation",
        "dispatch-assembly",
        json!({"work_order":order,"operation":operation,"work_center":center}),
    )
    .await;
    assert_eq!(
        write("start_operation", "start-unissued", build.clone()).await["code"],
        "MATERIAL_NOT_ISSUED"
    );
    apply(
        "reserve_material",
        "reserve-assembly",
        json!({"reservation_id":"RS-1","work_order":order,"lot_id":"LOT-1","quantity":"2"}),
    )
    .await;
    apply("pick_material", "pick-assembly", reservation.clone()).await;
    apply(
        "issue_material_to_operation",
        "issue-assembly",
        json!({"reservation_id":"RS-1","operation":operation}),
    )
    .await;
    apply("start_operation", "start-assembly", build.clone()).await;
    let mut report = json!({"work_order":order,"operation":operation,"quantity":1,"material_inputs":{"RS-1":"1"}});
    let before = cursor().await;
    assert_eq!(
        write("post_operation_report", "insufficient", report.clone()).await["code"],
        "MATERIAL_INPUTS_INCOMPLETE"
    );
    assert_eq!(cursor().await, before);
    report["material_inputs"]["RS-1"] = json!("2");
    let actual = apply("post_operation_report", "report-assembly", report.clone()).await;
    let stock = |location| {
        invoke(
            "get_stock_balance",
            json!({"item_id":"WIRE","location":location}),
        )
    };
    assert_eq!(stock("RAW-02").await["balance"]["on_hand"], "0");
    let undo_issue = json!({"movement_type":"issue","movement_id":"RS-1","reversal_id":"REV-I","reason":"incorrect issue"});
    assert_eq!(
        write(
            "reverse_inventory_movement",
            "blocked-issue",
            undo_issue.clone()
        )
        .await["code"],
        "MOVEMENT_HAS_DEPENDENCIES"
    );
    assert_eq!(
        write(
            "return_unused_material",
            "blocked-return",
            json!({"reservation_id":"RS-1","movement_id":"RET-1","quantity":"1","reason":"unused"})
        )
        .await["code"],
        "RETURN_EXCEEDS_UNUSED_QUANTITY"
    );
    let trace = invoke("assembly_trace", json!({"subject":"LOT-1"})).await;
    assert_eq!(
        trace["links"][0]["material_inputs"],
        actual["report"]["material_inputs"]
    );
    apply(
        "reverse_operation_report",
        "reverse-assembly-report",
        json!({"work_order":order,"report_id":actual["report"]["id"],"reason":"correct input"}),
    )
    .await;
    assert_eq!(
        invoke("assembly_trace", json!({"subject":"LOT-1"})).await["links"],
        json!([])
    );
    apply(
        "return_unused_material",
        "return-assembly",
        json!({"reservation_id":"RS-1","movement_id":"RET-1","quantity":"1","reason":"unused"}),
    )
    .await;
    assert_eq!(stock("RAW-02").await["balance"]["on_hand"], "1");
    apply("reverse_inventory_movement","reverse-assembly-return",json!({"movement_type":"return","movement_id":"RET-1","reversal_id":"REV-R","reason":"incorrect return"})).await;
    apply(
        "reverse_inventory_movement",
        "reverse-assembly-issue",
        undo_issue,
    )
    .await;
    assert_eq!(stock("RAW-02").await["balance"]["on_hand"], "2");
    apply(
        "create_stock_transfer",
        "draft-assembly",
        json!({"transfer_id":"TR-1","lot_id":"LOT-1",
        "source_location":"RAW-02","destination_location":"RAW-01","reason":"reposition"}),
    )
    .await;
    apply(
        "post_stock_transfer",
        "move-assembly",
        json!({"transfer_id":"TR-1"}),
    )
    .await;
    assert_eq!(stock("RAW-01").await["balance"]["on_hand"], "2");
    apply("reverse_inventory_movement","reverse-assembly-transfer",json!({"movement_type":"transfer","movement_id":"TR-1","reversal_id":"TR-2","reason":"incorrect move"})).await;
    assert_eq!(stock("RAW-02").await["balance"]["on_hand"], "2");
    let journals = invoke(
        "list_inventory_movements",
        json!({"movement_type":"operation_reversal"}),
    )
    .await;
    assert_eq!(journals["movements"].as_array().unwrap().len(), 2);
    apply(
        "reserve_material",
        "reserve-assembly-again",
        json!({"reservation_id":"RS-2","work_order":order,"lot_id":"LOT-1","quantity":"2"}),
    )
    .await;
    apply(
        "pick_material",
        "pick-assembly-again",
        json!({"reservation_id":"RS-2"}),
    )
    .await;
    apply(
        "issue_material_to_operation",
        "issue-assembly-again",
        json!({"reservation_id":"RS-2","operation":operation}),
    )
    .await;
    report["material_inputs"] = json!({"RS-2":"2"});
    apply("post_operation_report", "report-assembly-again", report).await;
    apply("complete_operation", "complete-assembly", build).await;
    assert_eq!(stock("RAW-02").await["balance"]["on_hand"], "0");
    apply("create_receipt","spare-receipt",json!({"receipt_id":"RC-2","item_id":"WIRE","revision":1,"quantity":"1","requires_inspection":false})).await;
    apply(
        "assign_receipt_tracking",
        "spare-track",
        json!({"receipt_id":"RC-2","lot_id":"LOT-2","supplier_lot":"SUP","location":"RAW-02"}),
    )
    .await;
    apply("post_receipt", "spare-post", json!({"receipt_id":"RC-2"})).await;
    apply("reverse_inventory_movement","reverse-assembly-receipt",json!({"movement_type":"receipt","movement_id":"RC-2","reversal_id":"REV-C","reason":"incorrect receipt"})).await;
    assert_eq!(stock("RAW-02").await["balance"]["on_hand"], "0");
    assert_eq!(
        invoke(
            "list_inventory_movements",
            json!({"movement_type":"receipt_reversal"})
        )
        .await["movements"][0]["receipt_id"],
        "RC-2"
    );
}

#[tokio::test]
async fn p9d_01_reported_wip_output() {
    operation_issue_scenarios(&[("cnc", 35, "WO-1", "machining", "MACH-01")]).await;
}

async fn reported_wip_output(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    order: &str,
    operation: &str,
) {
    let invoke = async |name: &str, args| business(client, run, name, args).await;
    let cursor = async || call(client, "get_run", json!({"run_id":run})).await["cursor"].clone();
    let write = async |name: &str, key: &str, mut args: Value| {
        args["idempotency_key"] = json!(key);
        args["expected_cursor"] = cursor().await;
        let result = invoke(name, args).await;
        assert_eq!(result["ok"], true, "{name}: {result}");
        result
    };
    let read = |order| invoke("get_wip", json!({"work_order":order}));
    assert_eq!(read(order).await["lots"], json!([]));
    write(
        "start_operation",
        "start-wip",
        json!({"work_order":order,"operation":operation}),
    )
    .await;
    let mut report = json!({"work_order":order,"operation":operation,"quantity":1,
        "material_inputs":{"RS-1":"0.625"},"idempotency_key":"wip-report-1","expected_cursor":cursor().await});
    let posted = invoke("post_operation_report", report.clone()).await;
    assert_eq!(posted["ok"], true, "{posted}");
    assert_eq!(
        invoke("post_operation_report", report.clone()).await,
        posted
    );
    report["material_inputs"]["RS-1"] = json!("0.375");
    let second = write("post_operation_report", "wip-report-2", report).await;
    let first_id = posted["report"]["id"].as_str().unwrap();
    let second_id = second["report"]["id"].as_str().unwrap();
    let at = cursor().await;
    let page = invoke("get_wip", json!({"work_order":order,"limit":1})).await;
    assert_eq!(page["next_after_wip_id"], first_id);
    assert_eq!(
        page["lots"][0],
        json!({"wip_id":first_id,"report_id":first_id,"work_order":order,"source_operation":operation,
        "unit":"each","quantity":1,"status":"available","material_inputs":posted["report"]["material_inputs"],
        "serial_numbers":posted["report"]["serial_numbers"]})
    );
    let following = invoke(
        "get_wip",
        json!({"work_order":order,"after_wip_id":first_id}),
    )
    .await;
    assert_eq!(following["lots"][0]["wip_id"], second_id);
    assert!(following["next_after_wip_id"].is_null());
    assert_eq!(
        invoke("get_wip", json!({"work_order":"WO-2","wip_id":first_id})).await["code"],
        "WIP_NOT_FOUND"
    );
    assert_eq!(cursor().await, at);
    write(
        "reverse_operation_report",
        "reverse-wip",
        json!({"work_order":order,"report_id":second_id,"reason":"correct report"}),
    )
    .await;
    let reversed = invoke("get_wip", json!({"work_order":order,"wip_id":second_id})).await;
    assert_eq!(reversed["lots"][0]["status"], "reversed");
    assert_eq!(
        reversed["lots"][0]["material_inputs"],
        second["report"]["material_inputs"]
    );
    write(
        "create_inspection",
        "wip-qc",
        json!({"inspection_id":"WIP-QC","work_order":order,"specification_version":"QA@1"}),
    )
    .await;
    write(
        "record_measurement",
        "wip-measure",
        json!({"inspection_id":"WIP-QC","characteristic":"width","unit":"mm",
        "value":3,"lower_limit":1,"upper_limit":2}),
    )
    .await;
    write(
        "complete_inspection",
        "wip-failed",
        json!({"inspection_id":"WIP-QC"}),
    )
    .await;
    let current = read(order).await;
    assert_eq!(current["lots"][0]["blocking"], "QUALITY_HOLD");
    assert_eq!(current["lots"][0]["status"], "held");
    assert_eq!(current["lots"][1]["status"], "reversed");
    assert_eq!(
        invoke(
            "get_stock_balance",
            json!({"item_id":"WIRE","location":"RAW-02"})
        )
        .await["balance"]["on_hand"],
        "0.5"
    );
}

#[test]
fn p9d_02_move_and_consume_wip() {
    run_wip_scenario(36);
}

#[test]
fn p9d_06_reverse_wip_movement() {
    run_wip_scenario(38);
}

#[test]
fn p9d_10_reconcile_work_order() {
    run_wip_scenario(40);
}

#[test]
fn p9d_14_reconcile_finished_work_order() {
    run_wip_scenario(42);
}

#[test]
fn p9d_12_receive_finished_output() {
    run_wip_scenario(41);
}

#[test]
fn p9d_08_split_merge_wip() {
    run_wip_scenario(39);
}

#[test]
fn p9d_04_batch_serial_genealogy() {
    run_wip_scenario(37);
}

fn run_wip_scenario(version: u64) {
    run_wip_scenario_with_quality(version, false);
}

fn run_wip_scenario_with_quality(version: u64, quality_recovery: bool) {
    // The full inventory, WIP and genealogy flow composes large debug-mode futures.
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(operation_issue_scenarios_with_quality(
                    &[("cnc", version, "WO-1", "machining", "MACH-01")],
                    quality_recovery,
                ));
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn move_and_consume_wip(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    order: &str,
    operation: &str,
    genealogy: bool,
    reversal: bool,
    version: u64,
    quality_recovery: bool,
) {
    let legacy_reconciliation = version == 40;
    let partition = matches!(version, 39 | 40);
    let invoke = async |name: &str, args| business(client, run, name, args).await;
    let cursor = async || call(client, "get_run", json!({"run_id":run})).await["cursor"].clone();
    let write = async |name: &str, key: &str, mut args: Value| {
        args["idempotency_key"] = json!(key);
        args["expected_cursor"] = cursor().await;
        invoke(name, args).await
    };
    let apply = async |name: &str, key: &str, args| {
        let result = write(name, key, args).await;
        assert_eq!(result["ok"], true, "{name}: {result}");
        result
    };
    let prep = json!({"work_order":order,"operation":operation});
    let assembly = json!({"work_order":order,"operation":"assembly"});
    if genealogy || version == 42 {
        let location = if version == 42 { "RAW-01" } else { "RAW-02" };
        Box::pin(setup_genealogy_second_lot(
            client, run, order, operation, location,
        ))
        .await;
    }
    apply("start_operation", "start-source", prep.clone()).await;
    let machining_report = |quantity, amount| json!({"work_order":order,"operation":operation,"quantity":quantity,"material_inputs":{"RS-1":amount}});
    let reconcile = || invoke("reconcile_work_order", json!({"work_order":order}));
    if legacy_reconciliation {
        assert_eq!(reconcile().await["wip"]["machined"], 0);
    }
    let first = apply(
        "post_operation_report",
        "source1",
        machining_report(4, "0.5"),
    )
    .await;
    let first_id = &first["report"]["id"];
    let mut movement = json!({"work_order":order,"wip_id":first_id,"destination_operation":"assembly","reason":"handoff"});
    assert_eq!(
        write("move_wip", "early", movement.clone()).await["code"],
        "WIP_DESTINATION_NOT_ELIGIBLE"
    );
    if legacy_reconciliation {
        apply(
            "return_unused_material",
            "return-before-assembly",
            json!({
                "reservation_id":"RS-1","movement_id":"RET-1","quantity":"0.25","reason":"surplus"
            }),
        )
        .await;
    }
    let mut second_args = machining_report(6, if legacy_reconciliation { "0.75" } else { "1" });
    if genealogy || version == 42 {
        second_args["material_inputs"] = json!({"RS-2":"1"});
    }
    let second = apply("post_operation_report", "source2", second_args).await;
    let second_id = &second["report"]["id"];
    apply("complete_operation", "complete-source", prep).await;
    apply(
        "dispatch_operation",
        "dispatch-destination",
        json!({"work_order":order,"operation":"assembly","work_center":"ASM-01"}),
    )
    .await;
    let before = cursor().await;
    assert_eq!(
        write("start_operation", "no-wip", assembly.clone()).await["code"],
        "WIP_NOT_AT_OPERATION"
    );
    assert_eq!(cursor().await, before);
    if partition {
        let mut split = json!({"work_order":order,"wip_id":second_id,"reason":"stage separate bins",
            "selected_serial_numbers":["UNKNOWN"]});
        let before = cursor().await;
        assert_eq!(
            write("split_wip", "unknown-serial", split.clone()).await["code"],
            "WIP_SERIAL_MISMATCH"
        );
        assert_eq!(cursor().await, before);
        split["selected_serial_numbers"] = json!([second["report"]["serial_numbers"][0]]);
        split["expected_cursor"] = before;
        split["idempotency_key"] = json!("split-second");
        let result = invoke("split_wip", split.clone()).await;
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(invoke("split_wip", split).await, result);
        assert_eq!(
            write(
                "move_wip",
                "move-split-parent",
                json!({"work_order":order,"wip_id":second_id,
            "destination_operation":"assembly","reason":"handoff"})
            )
            .await["code"],
            "WIP_NOT_AVAILABLE"
        );
        let child = &result["partition"]["children"][0]["id"];
        let sibling = &result["partition"]["children"][1]["id"];
        let read = async |id| {
            invoke("get_wip", json!({"work_order":order,"wip_id":id})).await["lots"][0].clone()
        };
        assert_eq!(read(second_id).await["status"], "split");
        assert_eq!(read(child).await["quantity"], 1);
        assert_eq!(read(sibling).await["quantity"], 5);
        let transfer = apply(
            "move_wip",
            "move-child",
            json!({"work_order":order,"wip_id":child,
            "destination_operation":"assembly","reason":"handoff"}),
        )
        .await;
        let mut merge = json!({"work_order":order,"wip_id":second_id,"reason":"recombine",
            "split_cursor":result["partition"]["split_cursor"]});
        assert_eq!(
            write("merge_wip", "merge-moved", merge.clone()).await["code"],
            "WIP_NOT_AVAILABLE"
        );
        apply("start_operation", "start-child-assembly", assembly.clone()).await;
        let child_report = apply(
            "post_operation_report",
            "consume-child",
            json!({"work_order":order,
            "operation":"assembly","quantity":1,"wip_inputs":[child]}),
        )
        .await;
        assert_eq!(
            child_report["report"]["serial_numbers"],
            read(child).await["serial_numbers"]
        );
        let withdraw = json!({"work_order":order,"wip_id":child,
            "movement_cursor":transfer["movement"]["cursor"],"reason":"return for consolidation"});
        assert_eq!(
            write(
                "reverse_wip_movement",
                "withdraw-consumed",
                withdraw.clone()
            )
            .await["code"],
            "WIP_ALREADY_CONSUMED"
        );
        apply(
            "reverse_operation_report",
            "reverse-child-report",
            json!({"work_order":order,
            "report_id":child_report["report"]["id"],"reason":"undo child assembly"}),
        )
        .await;
        apply("reverse_wip_movement", "withdraw-child", withdraw).await;
        merge["expected_cursor"] = cursor().await;
        merge["idempotency_key"] = json!("merge-siblings");
        let merged = invoke("merge_wip", merge.clone()).await;
        assert_eq!(merged["ok"], true, "{merged}");
        assert_eq!(invoke("merge_wip", merge.clone()).await, merged);
        assert_eq!(
            write("merge_wip", "merge-old", merge).await["code"],
            "WIP_SPLIT_NOT_ACTIVE"
        );
        assert_eq!(read(child).await["status"], "merged");
        assert_eq!(read(sibling).await["status"], "merged");
        assert_eq!(read(second_id).await["quantity"], 6);
        if legacy_reconciliation {
            let check = reconcile().await;
            assert_eq!(check["balanced"], true, "{check}");
            assert_eq!(
                check["wip"],
                json!({"machined":10,"at_source":10,"at_destination":0,"consumed":0,"assembly_reported":0})
            );
        }

        let edges = invoke(
            "list_genealogy_edges",
            json!({"work_order":order,"limit":100}),
        )
        .await;
        let edges = edges["edges"].as_array().unwrap();
        assert!(
            edges
                .iter()
                .any(|edge| edge["kind"] == "wip_split" && edge["target"]["id"] == *child)
        );
        assert!(
            edges
                .iter()
                .any(|edge| edge["kind"] == "wip_merge" && edge["source"]["id"] == *child)
        );
        assert!(edges.iter().any(|edge| edge["kind"] == "wip_consumption"
            && edge["source"]["id"] == *child
            && edge["status"] == "reversed"));
    }
    let mut wrong = movement.clone();
    wrong["destination_operation"] = json!("inspection");
    assert_eq!(
        write("move_wip", "skip", wrong).await["code"],
        "WIP_DESTINATION_NOT_ELIGIBLE"
    );
    apply(
        "hold_work_order",
        "hold",
        json!({"work_order":order,"reason":"supervisor hold"}),
    )
    .await;
    assert_eq!(
        write("move_wip", "held", movement.clone()).await["code"],
        "WORK_ORDER_HELD"
    );
    apply("resume_work_order", "resume", json!({"work_order":order})).await;
    movement["expected_cursor"] = cursor().await;
    movement["idempotency_key"] = json!("move-first");
    let moved = invoke("move_wip", movement.clone()).await;
    assert_eq!(moved["ok"], true, "{moved}");
    assert_eq!(invoke("move_wip", movement.clone()).await, moved);
    assert_eq!(
        write("move_wip", "duplicate", movement.clone()).await["code"],
        "WIP_NOT_AVAILABLE"
    );
    assert_eq!(
        write(
            "reverse_operation_report",
            "reverse-source",
            json!({"work_order":order,"report_id":first_id,"reason":"incorrect source"})
        )
        .await["code"],
        "REPORT_HAS_DEPENDENCIES"
    );
    if !partition {
        apply("start_operation", "start-destination", assembly.clone()).await;
    }
    let mut report = json!({"work_order":order,"operation":"assembly","quantity":4});
    assert_eq!(
        write("post_operation_report", "missing", report.clone()).await["code"],
        "WIP_INPUTS_REQUIRED"
    );
    report["wip_inputs"] = json!([first_id, first_id]);
    assert_eq!(
        write("post_operation_report", "duplicates", report.clone()).await["code"],
        "INVALID_ARGUMENT"
    );
    report["wip_inputs"] = json!([second_id]);
    assert_eq!(
        write("post_operation_report", "not-moved", report.clone()).await["code"],
        "WIP_NOT_AT_OPERATION"
    );
    report["wip_inputs"] = json!([first_id]);
    report["quantity"] = json!(3);
    assert_eq!(
        write("post_operation_report", "wrong-quantity", report.clone()).await["code"],
        "WIP_QUANTITY_MISMATCH"
    );
    report["quantity"] = json!(4);
    let consumed = apply("post_operation_report", "consume", report.clone()).await;
    assert_eq!(
        consumed["report"]["serial_numbers"],
        first["report"]["serial_numbers"]
    );
    assert_eq!(
        write("post_operation_report", "consumed-again", report.clone()).await["code"],
        "WIP_ALREADY_CONSUMED"
    );
    let read = || invoke("get_wip", json!({"work_order":order,"wip_id":first_id}));
    assert_eq!(
        read().await["lots"][0]["consumed_by"],
        consumed["report"]["id"]
    );
    let mut withdraw = json!({"work_order":order,"wip_id":first_id,
        "movement_cursor":moved["movement"]["cursor"],"reason":"incorrect handoff"});
    if reversal {
        let before = cursor().await;
        assert_eq!(
            write("reverse_wip_movement", "consumed-move", withdraw.clone()).await["code"],
            "WIP_ALREADY_CONSUMED"
        );
        assert_eq!(cursor().await, before);
    }
    apply("reverse_operation_report","reverse-assembly",json!({"work_order":order,"report_id":consumed["report"]["id"],"reason":"incorrect report"})).await;
    assert_eq!(read().await["lots"][0]["status"], "moved");
    if legacy_reconciliation {
        let check = reconcile().await;
        assert_eq!(
            check["wip"],
            json!({"machined":10,"at_source":6,"at_destination":4,"consumed":0,"assembly_reported":0})
        );
        assert_eq!(check["balanced"], true, "{check}");
    }

    if reversal {
        withdraw["expected_cursor"] = cursor().await;
        withdraw["idempotency_key"] = json!("withdraw");
        let reversed = invoke("reverse_wip_movement", withdraw.clone()).await;
        assert_eq!(
            reversed["reversal"]["original_movement"], moved["movement"],
            "{reversed}"
        );
        assert_eq!(
            invoke("reverse_wip_movement", withdraw.clone()).await,
            reversed
        );
        let current = read().await;
        assert_eq!(current["lots"][0]["status"], "available");
        assert!(current["lots"][0].get("movement").is_none());
        assert_eq!(
            current["lots"][0]["movement_reversals"],
            json!([reversed["reversal"]])
        );
        assert_eq!(
            write("post_operation_report", "withdrawn", report.clone()).await["code"],
            "WIP_NOT_AT_OPERATION"
        );
        apply("move_wip", "move-again", movement.clone()).await;
        assert_eq!(
            write("reverse_wip_movement", "old-move", withdraw).await["code"],
            "WIP_MOVEMENT_NOT_ACTIVE"
        );
    }
    if version == 42 {
        let check =
            invoke("reconcile_work_order", json!({"work_order":order})).await["settlement"].clone();
        assert_eq!(check["production"]["wip_open"], 10);
        assert_eq!(check["production"]["assembled"], 0);
        assert_eq!(
            check["production"]["reversed_report_ids"],
            json!([consumed["report"]["id"]])
        );
        assert_eq!(check["differences"], json!([]));
    }
    let reconsumed = apply("post_operation_report", "consume-again", report.clone()).await;
    assert_eq!(
        reconsumed["report"]["serial_numbers"],
        consumed["report"]["serial_numbers"]
    );
    movement["wip_id"] = second_id.clone();
    apply("move_wip", "move-second", movement).await;
    report["quantity"] = json!(6);
    report["wip_inputs"] = json!([second_id]);
    apply("post_operation_report", "consume-second", report).await;
    apply("complete_operation", "complete-assembly", assembly).await;
    if version == 42 {
        apply(
            "return_unused_material",
            "return-surplus",
            json!({"reservation_id":"RS-1","movement_id":"RET-RECON",
            "quantity":"1","reason":"unused balance"}),
        )
        .await;
        apply("reverse_inventory_movement","reverse-surplus-return",json!({"movement_type":"return",
            "movement_id":"RET-RECON","reversal_id":"REV-RET-RECON","reason":"incorrect return posting"})).await;
        let check =
            invoke("reconcile_work_order", json!({"work_order":order})).await["settlement"].clone();
        assert_eq!(check["material_issues"][0]["returned"], "0");
        assert_eq!(check["material_issues"][0]["unused"], "1");
        apply(
            "return_unused_material",
            "return-surplus-again",
            json!({"reservation_id":"RS-1","movement_id":"RET-RECON-2",
            "quantity":"1","reason":"corrected return"}),
        )
        .await;
    }
    if version >= 41 {
        Box::pin(receive_finished_output(
            client,
            run,
            order,
            &first["report"]["serial_numbers"][0],
            version == 42,
            quality_recovery,
        ))
        .await;
    }
    if legacy_reconciliation {
        let before = cursor().await;
        let check = reconcile().await;
        assert_eq!(check["balanced"], true, "{check}");
        assert_eq!(
            check["wip"],
            json!({"machined":10,"at_source":0,"at_destination":0,"consumed":10,"assembly_reported":10})
        );
        assert_eq!(check["issues"][0]["issued"], "1.5");
        assert_eq!(check["issues"][0]["returned"], "0.25");
        assert_eq!(check["issues"][0]["consumed"], "1.25");
        assert_eq!(check["issues"][0]["unconsumed"], "0");
        assert_eq!(cursor().await, before);
        assert_eq!(
            invoke("reconcile_work_order", json!({"work_order":"unknown"})).await["code"],
            "WORK_ORDER_NOT_FOUND"
        );
    }

    if genealogy {
        Box::pin(assert_genealogy(
            client,
            run,
            order,
            &second["report"]["serial_numbers"][0],
            &consumed["report"]["id"],
            &reconsumed["report"]["id"],
        ))
        .await;
    }
    assert_eq!(
        invoke(
            "get_stock_balance",
            json!({"item_id":"WIRE","location":"RAW-02"})
        )
        .await["balance"]["on_hand"],
        match version {
            42 => "1.5",
            40 => "0.75",
            _ => "0.5",
        }
    );
}

#[test]
fn p9d_16_quality_recovery_before_output() {
    run_wip_scenario_with_quality(42, true);
}

async fn recover_output_quality(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    order: &str,
    receipt: &Value,
) {
    let invoke = async |name: &str, args| business(client, run, name, args).await;
    let cursor = async || call(client, "get_run", json!({"run_id":run})).await["cursor"].clone();
    let write = async |name: &str, key: &str, mut args: Value| {
        args["expected_cursor"] = cursor().await;
        args["idempotency_key"] = json!(key);
        invoke(name, args).await
    };
    let check = async |id: &str, value: f64, prior: Option<&str>| {
        let mut args =
            json!({"work_order":order,"inspection_id":id,"specification_version":"QA@1"});
        if let Some(prior) = prior {
            args["reinspection_of"] = json!(prior);
        }
        let created = write("create_inspection", &format!("create-{id}"), args).await;
        assert_eq!(created["ok"], true, "{created}");
        let measured=write("record_measurement",&format!("measure-{id}"),json!({
            "inspection_id":id,"characteristic":"width","unit":"mm","value":value,"lower_limit":1,"upper_limit":2
        })).await;
        assert_eq!(measured["ok"], true, "{measured}");
        write(
            "complete_inspection",
            &format!("complete-{id}"),
            json!({"inspection_id":id}),
        )
        .await
    };
    let move_quality = async |name: &str, key: &str, mut args: Value| {
        args["inspection_id"] = json!("FINAL-FAIL");
        args["responsible"] = json!("QA-01");
        args["reason"] = json!("final dimensional check");
        args["evidence"] = json!("QA-LOG");
        write(name, key, args).await
    };
    assert_eq!(
        check("FINAL-OLD", 1.5, None).await["inspection"]["status"],
        "passed"
    );
    assert_eq!(
        check("FINAL-FAIL", 3.0, None).await["inspection"]["status"],
        "failed"
    );
    assert_eq!(
        move_quality("open_nonconformance", "final-open", json!({})).await["nonconformance"]["status"],
        "open"
    );
    assert_eq!(
        move_quality("place_quality_hold", "final-hold", json!({})).await["nonconformance"]["status"],
        "held"
    );
    let before = cursor().await;
    let gated = write(
        "receive_work_order_output",
        "recovered-output",
        receipt.clone(),
    )
    .await;
    assert_eq!(gated["code"], "QUALITY_HOLD", "{gated}");
    assert_eq!(cursor().await, before);
    let account = invoke("reconcile_work_order", json!({"work_order":order})).await;
    assert_eq!(account["settlement"]["quality"], "QUALITY_HOLD");
    assert_eq!(account["balanced"], true);
    assert_eq!(account["settlement"]["output_receipts"], json!([]));
    assert_eq!(
        invoke(
            "get_stock_balance",
            json!({"item_id":"FRAME","location":"RAW-02"})
        )
        .await["balance"]["on_hand"],
        "0"
    );
    assert_eq!(
        move_quality(
            "disposition_nonconformance",
            "stale-evidence",
            json!({
                "decision":"reinspection","reinspection_id":"FINAL-OLD"
            })
        )
        .await["code"],
        "REINSPECTION_NOT_ACCEPTABLE"
    );
    assert_eq!(
        check("FINAL-PASS", 1.5, Some("FINAL-FAIL")).await["inspection"]["status"],
        "passed"
    );
    assert_eq!(
        move_quality(
            "disposition_nonconformance",
            "corrected-evidence",
            json!({
                "decision":"reinspection","reinspection_id":"FINAL-PASS"
            })
        )
        .await["nonconformance"]["status"],
        "disposed"
    );
    assert_eq!(
        write(
            "receive_work_order_output",
            "recovered-output",
            receipt.clone()
        )
        .await["code"],
        "QUALITY_HOLD"
    );
    assert_eq!(
        move_quality("release_quality_hold", "final-release", json!({})).await["nonconformance"]["status"],
        "released"
    );
    let history = invoke("get_inspection", json!({"inspection_id":"FINAL-FAIL"})).await;
    assert_eq!(history["inspection"]["status"], "failed");
    assert_eq!(
        history["nonconformance"]["history"][2]["reinspection_id"],
        "FINAL-PASS"
    );
    let available = invoke("reconcile_work_order", json!({"work_order":order})).await;
    assert_eq!(available["settlement"]["quality"], "released");
    assert_eq!(available["wip"], account["wip"]);
    assert_eq!(available["issues"], account["issues"]);
}

async fn receive_finished_output(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    order: &str,
    serial: &Value,
    reconciliation: bool,
    quality_recovery: bool,
) {
    let invoke = async |name: &str, args| business(client, run, name, args).await;
    let cursor = async || call(client, "get_run", json!({"run_id":run})).await["cursor"].clone();
    let write = async |name: &str, key: &str, mut args: Value| {
        args["idempotency_key"] = json!(key);
        args["expected_cursor"] = cursor().await;
        invoke(name, args).await
    };
    let apply = async |name: &str, key: &str, args| {
        let result = write(name, key, args).await;
        assert_eq!(result["ok"], true, "{name}: {result}");
        result
    };
    let mut receipt =
        json!({"work_order":order,"receipt_id":"FG-1","lot_id":"FG-LOT-1","location":"RAW-02"});
    let reversal = |id, key| json!({"movement_type":"receipt","movement_id":id,"reversal_id":key,"reason":"correct posting"});
    let balance = async || {
        invoke(
            "get_stock_balance",
            json!({"item_id":"FRAME","revision":1,"location":"RAW-02"}),
        )
        .await["balance"]["on_hand"]
            .clone()
    };
    let order_args = json!({"work_order":order});
    let before = cursor().await;
    assert_eq!(
        write("receive_work_order_output", "too-early", receipt.clone()).await["code"],
        "COMPLETION_REQUIRED"
    );
    assert_eq!(cursor().await, before);
    apply(
        "dispatch_operation",
        "dispatch-output-inspection",
        json!({"work_order":order,"operation":"inspection","work_center":"QA-01"}),
    )
    .await;
    apply(
        "inspect_units",
        "reject-output",
        json!({"work_order":order,"result":"fail"}),
    )
    .await;
    assert_eq!(
        write(
            "complete_work_order",
            "complete-rejected",
            order_args.clone()
        )
        .await["code"],
        "QUALITY_HOLD"
    );
    if reconciliation {
        assert_eq!(
            invoke("reconcile_work_order", json!({"work_order":"bad id"})).await["code"],
            "INVALID_ARGUMENT"
        );
        let check = invoke("reconcile_work_order", order_args.clone()).await["settlement"].clone();
        assert_eq!(check["quality"], "QUALITY_HOLD");
        assert_eq!(
            check["blocking"],
            json!(["QUALITY_HOLD", "COMPLETION_REQUIRED"])
        );
        assert_eq!(check["differences"], json!([]));
    }
    apply(
        "disposition",
        "release-output",
        json!({"work_order":order,"result":"pass","reason":"verified repeat inspection"}),
    )
    .await;
    apply(
        "complete_work_order",
        "confirm-finished",
        order_args.clone(),
    )
    .await;
    assert_eq!(
        write("close_work_order", "close-early", order_args.clone()).await["code"],
        "OUTPUT_RECEIPT_REQUIRED"
    );
    assert_eq!(balance().await, "0");
    apply(
        "create_item_revision",
        "draft-frame-v2",
        json!({"item_id":"FRAME","revision":2,"name":"Frame V2"}),
    )
    .await;
    apply(
        "activate_revision",
        "activate-frame-v2",
        json!({"item_id":"FRAME","revision":2,"revision_type":"item"}),
    )
    .await;
    if quality_recovery {
        Box::pin(recover_output_quality(client, run, order, &receipt)).await;
    }
    receipt["idempotency_key"] = json!(if quality_recovery {
        "recovered-output"
    } else {
        "post-fg"
    });
    receipt["expected_cursor"] = cursor().await;
    let posted = invoke("receive_work_order_output", receipt.clone()).await;
    assert_eq!(posted["ok"], true, "{posted}");
    assert_eq!(posted["receipt"]["revision"], 1);
    assert_eq!(posted["receipt"]["quantity"], "10");
    assert_eq!(
        posted["receipt"]["serial_numbers"]
            .as_array()
            .unwrap()
            .len(),
        10
    );
    assert_eq!(
        invoke("receive_work_order_output", receipt.clone()).await,
        posted
    );
    assert_eq!(
        write("receive_work_order_output", "duplicate-fg", receipt.clone()).await["code"],
        "OUTPUT_RECEIPT_EXISTS"
    );
    assert_eq!(balance().await, "10");
    assert_eq!(
        invoke("get_lot", json!({"lot_id":"FG-LOT-1"})).await["lot"],
        posted["receipt"]
    );
    assert_eq!(write("create_stock_transfer","split-serialized",json!({"transfer_id":"TR-FG","lot_id":"FG-LOT-1",
        "source_location":"RAW-02","destination_location":"RAW-01","child_lot_id":"FG-CHILD","quantity":"1","reason":"partial transfer"})).await["code"],"LOT_SPLIT_NOT_ALLOWED");
    apply(
        "reverse_inventory_movement",
        "correct-fg",
        reversal("FG-1", "REV-FG-1"),
    )
    .await;
    assert_eq!(balance().await, "0");
    assert_eq!(
        write(
            "close_work_order",
            "close-after-reversal",
            order_args.clone()
        )
        .await["code"],
        "OUTPUT_RECEIPT_REQUIRED"
    );
    if reconciliation {
        let check = invoke("reconcile_work_order", order_args.clone()).await["settlement"].clone();
        assert_eq!(check["production"]["received"], 0);
        assert_eq!(check["blocking"], json!(["OUTPUT_RECEIPT_REQUIRED"]));
        assert_eq!(check["output_receipts"][0]["reversed"], true);
        assert_eq!(check["output_receipts"][0]["lot_on_hand"], "0");
    }
    receipt["receipt_id"] = json!("FG-2");
    receipt["lot_id"] = json!("FG-LOT-2");
    let reposted = apply("receive_work_order_output", "post-fg-again", receipt).await;
    assert_eq!(
        reposted["receipt"]["serial_numbers"],
        posted["receipt"]["serial_numbers"]
    );
    let trace = async |name: &str, args| {
        let result = invoke(name, args).await;
        let edges = result["edges"].as_array().unwrap();
        assert!(edges.iter().any(|edge| edge["source"]["id"] == *serial
            && edge["kind"] == "finished_output_receipt"
            && edge["target"]["id"] == "FG-LOT-1"
            && edge["status"] == "reversed"));
        assert!(edges.iter().any(|edge| edge["source"]["id"] == *serial
            && edge["kind"] == "finished_output_receipt"
            && edge["target"]["id"] == "FG-LOT-2"
            && edge["status"] == "active"));
    };
    trace("trace_serial_backward", json!({"serial_number":serial})).await;
    trace("trace_lot_forward", json!({"lot_id":"LOT-1"})).await;
    apply("close_work_order", "close-finished", order_args.clone()).await;
    assert_eq!(
        write(
            "reverse_inventory_movement",
            "reverse-settled",
            reversal("FG-2", "REV-FG-2")
        )
        .await["code"],
        "RECEIPT_HAS_DEPENDENCIES"
    );
    if reconciliation {
        let before = cursor().await;
        let check = invoke("reconcile_work_order", order_args.clone()).await["settlement"].clone();
        assert_eq!(check["closed"], true, "{check}");
        assert_eq!(check["quality"], "released");
        assert_eq!(
            check["snapshot_fingerprint"],
            invoke("get_order_snapshot", order_args.clone()).await["snapshot"]["fingerprint"]
        );
        assert_eq!(check["cursor"], before);
        assert_eq!(cursor().await, before);
        assert_eq!(
            invoke("reconcile_work_order", order_args).await["settlement"],
            check
        );
        assert_eq!(check["differences"], json!([]));
        assert_eq!(check["blocking"], json!([]));
        assert_eq!(check["production"]["machined"], 10);
        assert_eq!(check["production"]["assembled"], 10);
        assert_eq!(check["production"]["wip_open"], 0);
        assert_eq!(check["production"]["wip_consumed"], 10);
        assert_eq!(check["production"]["received"], 10);
        let rows = check["material_issues"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["issue_location"], "RAW-02");
        assert_eq!(rows[0]["issued"], "1.5");
        assert_eq!(rows[0]["returned"], "1");
        assert_eq!(rows[0]["net_issued"], "0.5");
        assert_eq!(rows[0]["consumed"], "0.5");
        assert_eq!(rows[0]["unused"], "0");
        assert_eq!(rows[0]["lot_on_hand"], "1.5");
        assert_eq!(rows[1]["issue_location"], "RAW-01");
        assert_eq!(rows[1]["consumed"], "1");
        assert_eq!(check["output_receipts"][1]["lot_on_hand"], "10");
        let other = invoke("reconcile_work_order", json!({"work_order":"WO-2"})).await;
        assert_eq!(other["settlement"]["material_issues"], json!([]));
        assert_eq!(other["settlement"]["output_receipts"], json!([]));
    }
    assert_eq!(balance().await, "10");
}

async fn setup_genealogy_second_lot(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    order: &str,
    operation: &str,
    location: &str,
) {
    let apply = async |name: &str, key: &str, mut args: Value| {
        args["idempotency_key"] = json!(key);
        args["expected_cursor"] =
            call(client, "get_run", json!({"run_id":run})).await["cursor"].clone();
        let result = business(client, run, name, args).await;
        assert_eq!(result["ok"], true, "{name}: {result}");
    };
    apply("create_receipt","second-receipt",json!({"receipt_id":"RC-2","item_id":"WIRE","revision":1,"quantity":"1","requires_inspection":false})).await;
    apply(
        "assign_receipt_tracking",
        "second-tracking",
        json!({"receipt_id":"RC-2","lot_id":"LOT-2","supplier_lot":"SUP-2","location":location}),
    )
    .await;
    apply("post_receipt", "second-post", json!({"receipt_id":"RC-2"})).await;
    apply(
        "reserve_material",
        "second-reserve",
        json!({"reservation_id":"RS-2","work_order":order,"lot_id":"LOT-2","quantity":"1"}),
    )
    .await;
    apply(
        "pick_material",
        "second-pick",
        json!({"reservation_id":"RS-2"}),
    )
    .await;
    apply(
        "issue_material_to_operation",
        "second-issue",
        json!({"reservation_id":"RS-2","operation":operation}),
    )
    .await;
}
async fn assert_genealogy(
    client: &RunningService<RoleClient, ()>,
    run: &Value,
    order: &str,
    serial: &Value,
    reversed_report: &Value,
    active_report: &Value,
) {
    let invoke = async |name: &str, args| business(client, run, name, args).await;
    let cursor = async || call(client, "get_run", json!({"run_id":run})).await["cursor"].clone();
    let stable_cursor = cursor().await;
    let tools = call(
        client,
        "search_tools",
        json!({"run_id":run,"query":"genealogy"}),
    )
    .await;
    for name in [
        "trace_lot_forward",
        "trace_serial_backward",
        "list_genealogy_edges",
    ] {
        assert!(
            tools["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name)
        );
    }
    let forward = invoke("trace_lot_forward", json!({"lot_id":"LOT-1"})).await;
    let edges = forward["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 7, "{forward}");
    assert!(
        edges
            .iter()
            .any(|edge| edge["target"]["id"] == *reversed_report && edge["status"] == "reversed")
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge["target"]["id"] == *active_report && edge["status"] == "active")
    );
    assert!(edges.iter().all(|edge| edge["source"]["id"] != "LOT-2"));
    let backward = invoke(
        "trace_serial_backward",
        json!({"serial_number":serial,"work_order":order}),
    )
    .await;
    let edges = backward["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 2, "{backward}");
    assert_eq!(edges[0]["source"]["id"], "LOT-2");
    assert_eq!(edges[1]["target"]["id"], *serial);
    let first_page = invoke(
        "list_genealogy_edges",
        json!({"work_order":order,"limit":1}),
    )
    .await;
    let next = invoke(
        "list_genealogy_edges",
        json!({"work_order":order,"limit":1,"after_edge_id":first_page["next_after_edge_id"]}),
    )
    .await;
    assert_ne!(
        first_page["edges"][0]["edge_id"],
        next["edges"][0]["edge_id"]
    );
    assert_eq!(
        invoke(
            "trace_lot_forward",
            json!({"lot_id":"LOT-1","work_order":"WO-2"})
        )
        .await["edges"],
        json!([])
    );
    let isolated = call(
        client,
        "open_scenario",
        json!({"scenario_id":"cnc","version":37}),
    )
    .await;
    assert_eq!(
        business(
            client,
            &isolated["run_id"],
            "trace_serial_backward",
            json!({"serial_number":serial})
        )
        .await["edges"],
        json!([])
    );
    assert_eq!(cursor().await, stable_cursor);
}
fn operation_issue_scenarios<'a>(
    scenarios: &'a [(&'a str, u64, &'a str, &'a str, &'a str)],
) -> std::pin::Pin<Box<impl std::future::Future<Output = ()> + 'a>> {
    Box::pin(operation_issue_scenarios_with_quality(scenarios, false))
}

async fn operation_issue_scenarios_with_quality(
    scenarios: &[(&str, u64, &str, &str, &str)],
    quality_recovery: bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("inventory.sqlite3");
    let (client, _child) = connect_mode(&db, "0", true).await;
    timeout(Duration::from_secs(30), Box::pin(async {
        for &(scenario,version,order,operation,center) in scenarios {
            let opened=call(&client,"open_scenario",json!({"scenario_id":scenario,"version":version})).await;
            assert_eq!(opened["ok"],true,"{opened}");
            let run=&opened["run_id"];
            let invoke=async |name:&str,args| business(&client,run,name,args).await;
            let cursor=async || call(&client,"get_run",json!({"run_id":run})).await["cursor"].clone();
            let write=async |name:&str,key:&str,mut args:Value| {
                args["expected_cursor"]=cursor().await;
                args["idempotency_key"]=json!(key);
                invoke(name,args).await
            };
            let apply=async |name:&str,key:&str,args| {
                let result=write(name,key,args).await;
                assert_eq!(result["ok"],true,"{name}: {result}");
                result
            };
            let mut route = json!({(operation):{"work_center":center,"predecessors":[]}});
            if matches!(version,36..=42) {
                route["assembly"]=json!({"work_center":"ASM-01","predecessors":["machining"]});
            }
            if version >= 41 {
                route["inspection"]=json!({"work_center":"QA-01","predecessors":["assembly"]});
            }
            for (name,args) in [
                ("create_item",json!({"item_id":"FRAME","base_unit":"each"})),
                ("create_item",json!({"item_id":"WIRE","base_unit":"m"})),
                ("create_item_revision",json!({"item_id":"FRAME","revision":1,"name":"Frame"})),
                ("create_item_revision",json!({"item_id":"WIRE","revision":1,"name":"Wire"})),
                ("activate_revision",json!({"revision_type":"item","item_id":"FRAME","revision":1})),
                ("activate_revision",json!({"revision_type":"item","item_id":"WIRE","revision":1})),
                ("create_bom_revision",json!({"item_id":"FRAME","revision":1})),
                ("set_bom_components",json!({"item_id":"FRAME","revision":1,"components":{"WIRE":{"revision":1,"quantity":2,"unit":"m"}}})),
                ("activate_revision",json!({"revision_type":"bom","item_id":"FRAME","revision":1})),
                ("create_route_revision",json!({"item_id":"FRAME","revision":1})),
                ("set_route_operations",json!({"item_id":"FRAME","revision":1,"operations":route})),
                ("activate_revision",json!({"revision_type":"route","item_id":"FRAME","revision":1})),
                ("create_inspection_plan_revision",json!({"plan_id":"QA","revision":1,"characteristics":{"width":{"unit":"mm","lower_limit":1,"upper_limit":2}}})),
                ("activate_inspection_plan_revision",json!({"plan_id":"QA","revision":1})),
            ].into_iter().enumerate().map(|(i,(name,args))| (format!("setup-{i}"),(name,args))) {
                apply(args.0,&name,args.1).await;
            }
            let mut args=json!({"work_order":order,"definition_selection":{"item_id":"FRAME","inspection_plan_id":"QA"}});
            let readiness=invoke("get_release_readiness",args.clone()).await;
            assert_eq!(readiness["ready"],true,"{readiness}");
            args["snapshot_fingerprint"]=readiness["definition_snapshot"]["fingerprint"].clone();
            apply("release_work_order",order,args).await;

            apply("create_receipt","receipt",json!({"receipt_id":"RC-1","item_id":"WIRE","revision":1,"quantity":"2","requires_inspection":false})).await;
            apply("assign_receipt_tracking","track",json!({"receipt_id":"RC-1","lot_id":"LOT-1","supplier_lot":"SUP","location":"RAW-02"})).await;
            apply("post_receipt","post",json!({"receipt_id":"RC-1"})).await;
            let reserve=json!({"reservation_id":"RS-1","work_order":order,"lot_id":"LOT-1","quantity":"1.5"});
            if (scenario == "cnc" && version == 34) || (scenario == "assembly" && version == 28) {
                Box::pin(partial_lot_transfer(&client,run,order,operation,center)).await;
                continue;
            }
            if scenario == "assembly" && version == 27 {
                Box::pin(assembly_material_reconciliation(&client, run, order, operation, center)).await;
                continue;
            }
            if version == 32 {
                let described=call(&client,"describe_tool",json!({"run_id":run,"name":"reverse_inventory_movement"})).await;
                assert_eq!(described["tool"]["input"]["properties"]["movement_type"]["enum"],json!(["transfer","receipt"]));
                let mut request=json!({"movement_type":"receipt","movement_id":"RC-1","reversal_id":"REV-1","reason":"receipt entry error"});
                let history=invoke("list_inventory_movements",json!({"movement_type":"receipt"})).await;
                let stock=||invoke("get_stock_balance",json!({"item_id":"WIRE","location":"RAW-02"}));
                assert_eq!(stock().await["balance"]["on_hand"],"2");
                apply("create_stock_transfer","draft",json!({"transfer_id":"TR-1","lot_id":"LOT-1",
                    "source_location":"RAW-02","destination_location":"RAW-01","reason":"reposition"})).await;
                apply("reserve_material","reserved",reserve.clone()).await;
                let before=cursor().await;
                assert_eq!(write("reverse_inventory_movement","in-use",request.clone()).await["code"],"RECEIPT_HAS_DEPENDENCIES");
                assert_eq!(cursor().await,before);
                apply("release_reservation","release",json!({"reservation_id":"RS-1"})).await;
                request["expected_cursor"]=cursor().await;
                request["idempotency_key"]=json!("reverse-receipt");
                let result=invoke("reverse_inventory_movement",request.clone()).await;
                assert_eq!(result["ok"],true,"{result}");
                assert_eq!(result["receipt_reversal"]["quantity"],"2");
                assert_eq!(result["receipt_reversal"]["location"],"RAW-02");
                assert_eq!(invoke("reverse_inventory_movement",request.clone()).await,result);
                assert_eq!(write("reverse_inventory_movement","duplicate",request.clone()).await["code"],"RECEIPT_ALREADY_REVERSED");
                assert_eq!(stock().await["balance"]["on_hand"],"0");
                assert_eq!(stock().await["balance"]["available"],"0");
                assert_eq!(invoke("get_lot",json!({"lot_id":"LOT-1"})).await["code"],"LOT_NOT_FOUND");
                assert_eq!(invoke("list_lots",json!({})).await["lots"],json!([]));
                assert_eq!(invoke("list_inventory_movements",json!({"movement_type":"receipt"})).await,history);
                assert_eq!(write("post_stock_transfer","post-after-reverse",json!({"transfer_id":"TR-1"})).await["code"],"LOT_NOT_FOUND");
                let mut attempted=reserve;
                attempted["reservation_id"]=json!("RS-2");
                assert_eq!(write("reserve_material","reserve-after-reverse",attempted).await["code"],"LOT_NOT_FOUND");
                apply("create_receipt","pending-receipt",json!({"receipt_id":"RC-2","item_id":"WIRE","revision":1,"quantity":"1","requires_inspection":true})).await;
                request["movement_id"]=json!("RC-2");
                assert_eq!(write("reverse_inventory_movement","unposted",request.clone()).await["code"],"RECEIPT_NOT_POSTED");
                apply("assign_receipt_tracking","pending-track",json!({"receipt_id":"RC-2","lot_id":"LOT-2","supplier_lot":"SUP","location":"RAW-02"})).await;
                apply("post_receipt","pending-post",json!({"receipt_id":"RC-2"})).await;
                assert_eq!(stock().await["balance"]["pending_inspection"],"1");
                assert_eq!(write("reverse_inventory_movement","id-taken",request.clone()).await["code"],"REVERSAL_ID_ALREADY_EXISTS");
                request["reversal_id"]=json!("REV-2");
                apply("reverse_inventory_movement","pending-reverse",request).await;
                assert_eq!(stock().await["balance"]["pending_inspection"],"0");
                let (peer,_child)=connect_mode(&db,"0",true).await;
                let page=business(&peer,run,"list_inventory_movements",json!({"movement_type":"receipt_reversal","limit":1,"location":"RAW-02"})).await;
                assert_eq!(page["movements"],json!([result["receipt_reversal"]]));
                assert_eq!(page["next_after_movement_id"],"REV-1");
                let next=invoke("list_inventory_movements",json!({"movement_type":"receipt_reversal","after_movement_id":"REV-1"})).await;
                assert_eq!(next["movements"][0]["reversal_id"],"REV-2");
                assert!(next["next_after_movement_id"].is_null());
                apply("create_receipt","inspected-receipt",json!({"receipt_id":"RC-3","item_id":"WIRE","revision":1,"quantity":"1","requires_inspection":true})).await;
                apply("assign_receipt_tracking","inspected-track",json!({"receipt_id":"RC-3","lot_id":"LOT-3","supplier_lot":"SUP","location":"RAW-02"})).await;
                apply("post_receipt","inspected-post",json!({"receipt_id":"RC-3"})).await;
                apply("create_inspection","receipt-inspection",json!({"work_order":order,"inspection_id":"IN-1","specification_version":"QA@1","receipt_id":"RC-3"})).await;
                let before=cursor().await;
                assert_eq!(write("reverse_inventory_movement","inspection-blocked",json!({"movement_type":"receipt","movement_id":"RC-3","reversal_id":"REV-3","reason":"correct entry"})).await["code"],"RECEIPT_HAS_DEPENDENCIES");
                assert_eq!(cursor().await,before);
                assert_eq!(stock().await["balance"]["pending_inspection"],"1");

                continue;
            }
            if (30..=31).contains(&version) {
                let search=call(&client,"search_tools",json!({"run_id":run})).await;
                for name in ["create_stock_transfer","post_stock_transfer"] {
                    assert!(search["tools"].as_array().unwrap().iter().any(|tool|tool["name"]==name));
                }
                let draft_args=json!({"transfer_id":"TR-1","lot_id":"LOT-1","source_location":"RAW-02",
                    "destination_location":"RAW-01","reason":"reposition"});
                let draft=apply("create_stock_transfer","draft",draft_args.clone()).await;
                assert_eq!(draft["transfer"]["status"],"draft");
                let mut competing=draft_args;
                competing["transfer_id"]=json!("TR-2");
                apply("create_stock_transfer","draft-competing",competing).await;
                let balance=|location:Option<&str>| invoke("get_stock_balance",if let Some(location)=location {json!({"item_id":"WIRE","location":location})} else {json!({"item_id":"WIRE"})});
                assert_eq!(balance(Some("RAW-02")).await["balance"]["on_hand"],"2");
                assert_eq!(balance(Some("RAW-01")).await["balance"]["on_hand"],"0");
                let post=json!({"transfer_id":"TR-1"});
                apply("reserve_material","reserved",reserve.clone()).await;
                let before=cursor().await;
                assert_eq!(write("post_stock_transfer","blocked",post.clone()).await["code"],"LOT_COMMITTED");
                assert_eq!(cursor().await,before);
                apply("release_reservation","release",json!({"reservation_id":"RS-1"})).await;
                let mut submitted=post;
                submitted["expected_cursor"]=cursor().await;
                submitted["idempotency_key"]=json!("transfer-post");
                let receipt=invoke("post_stock_transfer",submitted.clone()).await;
                assert_eq!(receipt["transfer"]["quantity"],"2","{receipt}");
                assert_eq!(invoke("post_stock_transfer",submitted).await,receipt);
                assert_eq!(write("post_stock_transfer","duplicate-post",json!({"transfer_id":"TR-1"})).await["code"],"TRANSFER_ALREADY_POSTED");
                assert_eq!(write("post_stock_transfer","stale-source",json!({"transfer_id":"TR-2"})).await["code"],"TRANSFER_SOURCE_CHANGED");
                assert_eq!(balance(Some("RAW-02")).await["balance"]["on_hand"],"0");
                assert_eq!(balance(Some("RAW-01")).await["balance"]["on_hand"],"2");
                assert_eq!(balance(None::<&str>).await["balance"]["on_hand"],"2");
                assert_eq!(invoke("get_lot",json!({"lot_id":"LOT-1"})).await["lot"]["location"],"RAW-01");
                assert_eq!(invoke("list_lots",json!({"location":"RAW-01"})).await["lots"][0]["lot_id"],"LOT-1");
                assert_eq!(invoke("list_inventory_movements",json!({"location":"RAW-02"})).await["movements"][0]["lot_id"],"LOT-1");
                let mut pick=reserve;
                pick["reservation_id"]=json!("RS-2");
                assert_eq!(apply("reserve_material","reserve-moved",pick).await["reservation"]["location"],"RAW-01");
                apply("pick_material","pick-moved",json!({"reservation_id":"RS-2"})).await;
                apply("dispatch_operation","dispatch-moved",json!({"work_order":order,"operation":operation,"work_center":center})).await;
                let issued=apply("issue_material_to_operation","issue-moved",json!({"reservation_id":"RS-2","operation":operation})).await;
                apply("create_stock_transfer","draft-back",json!({"transfer_id":"TR-3","lot_id":"LOT-1",
                    "source_location":"RAW-01","destination_location":"RAW-02","reason":"return to storage"})).await;
                assert_eq!(write("post_stock_transfer","issued-block",json!({"transfer_id":"TR-3"})).await["code"],"LOT_COMMITTED");
                let returned=apply("return_unused_material","returned",json!({"movement_id":"RET-1","reservation_id":"RS-2","quantity":"1.5","reason":"unused"})).await;
                if version == 31 {
                    assert_eq!(write("reverse_inventory_movement","dependent-issue",json!({
                        "movement_type":"transfer","movement_id":"TR-1","reversal_id":"TR-5","reason":"dependent issue"
                    })).await["code"],"TRANSFER_HAS_DEPENDENCIES");
                }
                apply("post_stock_transfer","posted-back",json!({"transfer_id":"TR-3"})).await;
                assert_eq!(invoke("list_inventory_movements",json!({"movement_type":"issue","location":"RAW-01"})).await["movements"],json!([issued["reservation"]]));
                assert_eq!(invoke("list_inventory_movements",json!({"movement_type":"return","location":"RAW-01"})).await["movements"],json!([returned["movement"]]));
                let (peer,_child)=connect_mode(&db,"0",true).await;
                let page=business(&peer,run,"list_inventory_movements",json!({"movement_type":"transfer","limit":1,"location":"RAW-02"})).await;
                assert_eq!(page["movements"],json!([receipt["transfer"]]));
                assert_eq!(page["next_after_movement_id"],"TR-1");
                let next=invoke("list_inventory_movements",json!({"movement_type":"transfer","after_movement_id":"TR-1"})).await;
                assert_eq!(next["movements"][0]["transfer_id"],"TR-3");
                apply("create_receipt","pending-receipt",json!({"receipt_id":"RC-2","item_id":"WIRE","revision":1,"quantity":"1","requires_inspection":true})).await;
                apply("assign_receipt_tracking","pending-track",json!({"receipt_id":"RC-2","lot_id":"LOT-2","supplier_lot":"SUP","location":"RAW-02"})).await;
                apply("post_receipt","pending-post",json!({"receipt_id":"RC-2"})).await;
                apply("create_stock_transfer","pending-draft",json!({"transfer_id":"TR-4","lot_id":"LOT-2",
                    "source_location":"RAW-02","destination_location":"RAW-01","reason":"pending inspection"})).await;
                assert_eq!(write("post_stock_transfer","pending-transfer",json!({"transfer_id":"TR-4"})).await["code"],"LOT_PENDING_INSPECTION");
                if version == 31 {
                    let tool=call(&client,"describe_tool",json!({"run_id":run,"name":"reverse_inventory_movement"})).await;
                    assert_eq!(tool["tool"]["input"]["properties"]["movement_type"]["enum"],json!(["transfer"]));
                    let mut reversal=json!({"movement_type":"transfer","movement_id":"TR-3","reversal_id":"TR-5","reason":"wrong destination"});
                    let mut attempt=reversal.clone();
                    attempt["movement_type"]=json!("receipt");
                    assert_eq!(write("reverse_inventory_movement","type-invalid",attempt).await["code"],"INVALID_ARGUMENT");
                    for (movement,code) in [("TR-2","TRANSFER_NOT_POSTED"),("TR-1","TRANSFER_HAS_DEPENDENCIES"),("missing","TRANSFER_NOT_FOUND")] {
                        let mut attempt=reversal.clone();
                        attempt["movement_id"]=json!(movement);
                        let before=cursor().await;
                        assert_eq!(write("reverse_inventory_movement",movement,attempt).await["code"],code);
                        assert_eq!(cursor().await,before);
                    }
                    apply("reserve_material","reserved-again",json!({"reservation_id":"RS-3","work_order":order,"lot_id":"LOT-1","quantity":"1"})).await;
                    assert_eq!(write("reverse_inventory_movement","committed-reversal",reversal.clone()).await["code"],"LOT_COMMITTED");
                    apply("release_reservation","release-again",json!({"reservation_id":"RS-3"})).await;
                    reversal["expected_cursor"]=cursor().await;
                    reversal["idempotency_key"]=json!("reverse-transfer");
                    let result=invoke("reverse_inventory_movement",reversal.clone()).await;
                    assert_eq!(result["ok"],true,"{result}");
                    assert_eq!(result["transfer"]["reverses_transfer_id"],"TR-3");
                    assert_eq!(result["transfer"]["source_location"],"RAW-02");
                    assert_eq!(result["transfer"]["destination_location"],"RAW-01");
                    assert_eq!(result["transfer"]["quantity"],"2");
                    assert_eq!(invoke("reverse_inventory_movement",reversal.clone()).await,result);
                    reversal["reversal_id"]=json!("TR-6");
                    assert_eq!(write("reverse_inventory_movement","already-reversed",reversal.clone()).await["code"],"TRANSFER_ALREADY_REVERSED");
                    reversal["movement_id"]=json!("TR-5");
                    assert_eq!(write("reverse_inventory_movement","reverse-reversal",reversal).await["code"],"TRANSFER_IS_REVERSAL");
                    assert_eq!(balance(Some("RAW-01")).await["balance"]["on_hand"],"2");
                    assert_eq!(balance(Some("RAW-02")).await["balance"]["on_hand"],"1");
                    assert_eq!(balance(Some("RAW-02")).await["balance"]["pending_inspection"],"1");
                    let journal=business(&peer,run,"list_inventory_movements",json!({"movement_type":"transfer","after_movement_id":"TR-1"})).await;
                    assert_eq!(journal["movements"],json!([next["movements"][0],result["transfer"]]));
                    continue;
                }

                assert_eq!(invoke("hold_material",json!({"batch":"LOT-1","reason":"segregate","idempotency_key":"hold-transfer"})).await["ok"],true);
                assert_eq!(write("post_stock_transfer","held-transfer",json!({"transfer_id":"TR-2"})).await["code"],"MATERIAL_HELD");

                continue;
            }

            apply("reserve_material","reserve",reserve.clone()).await;
            let issue=json!({"reservation_id":"RS-1","operation":operation});
            assert_eq!(write("issue_material_to_operation","issue",issue.clone()).await["code"],"RESERVATION_STATE_INVALID");
            apply("pick_material","pick",json!({"reservation_id":"RS-1"})).await;
            assert_eq!(write("issue_material_to_operation","issue",issue.clone()).await["code"],"ISSUE_OPERATION_NOT_ELIGIBLE");
            apply("dispatch_operation","dispatch",json!({"work_order":order,"operation":operation,"work_center":center})).await;
            assert_eq!(write("issue_material_to_operation","issue",json!({"reservation_id":"RS-1","operation":"missing"})).await["code"],"ISSUE_OPERATION_NOT_FOUND");
            let balance=||invoke("get_stock_balance",json!({"item_id":"WIRE","location":"RAW-02"}));
            assert_eq!(balance().await["balance"]["on_hand"],"2");
            assert_eq!(balance().await["balance"]["reserved"],"1.5");
            let mut submitted=issue.clone();
            submitted["expected_cursor"]=cursor().await;
            submitted["idempotency_key"]=json!("issue");
            let first=invoke("issue_material_to_operation",submitted.clone()).await;
            assert_eq!(first["reservation"]["status"],"issued","{first}");
            assert_eq!(first["reservation"]["work_center"],center);
            assert_eq!(balance().await["balance"]["on_hand"],"0.5");
            assert_eq!(balance().await["balance"]["available"],"0.5");
            assert_eq!(balance().await["balance"]["reserved"],"0");
            assert_eq!(invoke("issue_material_to_operation",submitted).await,first);
            assert_eq!(write("issue_material_to_operation","duplicate",issue).await["code"],"RESERVATION_STATE_INVALID");
            if matches!(version,36..=42) {
                Box::pin(move_and_consume_wip(&client,run,order,operation,version==37,version==38,version,quality_recovery)).await;
                let (peer,_child)=connect_mode(&db,"0",true).await;
                let lots=business(&peer,run,"get_wip",json!({"work_order":order})).await;
                let lots=lots["lots"].as_array().unwrap();
                assert_eq!(lots.iter().filter(|lot|lot["status"]=="consumed").count(),2);
                assert_eq!(lots.iter().filter(|lot|lot["status"]=="merged").count(),if matches!(version,39 | 40) {2} else {0});
                assert_eq!(lots.len(),if matches!(version,39 | 40) {4} else {2});
                continue;
            }
            if version == 35 {
                Box::pin(reported_wip_output(&client,run,order,operation)).await;
                let (peer,_child)=connect_mode(&db,"0",true).await;
                let entries=business(&peer,run,"get_wip",json!({"work_order":order})).await;
                assert_eq!(entries["lots"][0]["status"],"held");
                assert_eq!(entries["lots"][1]["status"],"reversed");
                continue;
            }
            if version == 33 {
                let issue_reversal=json!({"movement_type":"issue","movement_id":"RS-1","reversal_id":"REV-ISSUE","reason":"incorrect issue"});
                apply("start_operation","start",json!({"work_order":order,"operation":operation})).await;
                let report=json!({"work_order":order,"operation":operation,"quantity":1,"material_inputs":{"RS-1":"0.25"}});
                let reported=apply("post_operation_report","report",report.clone()).await;
                assert_eq!(write("reverse_inventory_movement","report-dependent",issue_reversal.clone()).await["code"],"MOVEMENT_HAS_DEPENDENCIES");
                apply("reverse_operation_report","undo-report",json!({"work_order":order,"report_id":reported["report"]["id"],"reason":"correct issue"})).await;
                let returned=apply("return_unused_material","return-stock",json!({"movement_id":"RET-1","reservation_id":"RS-1","quantity":"0.5","reason":"unused"})).await;
                assert_eq!(balance().await["balance"]["on_hand"],"1");
                assert_eq!(write("reverse_inventory_movement","return-dependent",issue_reversal.clone()).await["code"],"MOVEMENT_HAS_DEPENDENCIES");
                apply("reserve_material","reserve-returned",json!({"reservation_id":"RS-2","work_order":order,"lot_id":"LOT-1","quantity":"0.75"})).await;
                let mut return_reversal=json!({"movement_type":"return","movement_id":"RET-1","reversal_id":"REV-RETURN","reason":"incorrect return"});
                let before=cursor().await;
                assert_eq!(write("reverse_inventory_movement","stock-committed",return_reversal.clone()).await["code"],"LOT_AVAILABLE_QUANTITY_EXCEEDED");
                assert_eq!(cursor().await,before);
                apply("release_reservation","release-returned",json!({"reservation_id":"RS-2"})).await;
                apply("create_receipt","replacement-receipt",json!({"receipt_id":"RC-2","item_id":"WIRE","revision":1,"quantity":"19","requires_inspection":false})).await;
                apply("assign_receipt_tracking","replacement-track",json!({"receipt_id":"RC-2","lot_id":"LOT-2","supplier_lot":"SUP","location":"RAW-01"})).await;
                apply("post_receipt","replacement-post",json!({"receipt_id":"RC-2"})).await;
                apply("reserve_material","replacement-reservation",json!({"reservation_id":"RS-3","work_order":order,"lot_id":"LOT-2","quantity":"19"})).await;
                assert_eq!(write("reverse_inventory_movement","over-order",return_reversal.clone()).await["code"],"ORDER_REQUIREMENT_EXCEEDED");
                apply("release_reservation","release-replacement",json!({"reservation_id":"RS-3"})).await;

                return_reversal["expected_cursor"]=cursor().await;
                return_reversal["idempotency_key"]=json!("reverse-return");
                let undo_return=invoke("reverse_inventory_movement",return_reversal.clone()).await;
                assert_eq!(undo_return["operation_reversal"]["quantity"],"0.5","{undo_return}");
                assert_eq!(invoke("reverse_inventory_movement",return_reversal.clone()).await,undo_return);
                assert_eq!(write("reverse_inventory_movement","duplicate-return",return_reversal).await["code"],"MOVEMENT_ALREADY_REVERSED");
                assert_eq!(balance().await["balance"]["on_hand"],"0.5");
                assert_eq!(invoke("get_material_requirements",json!({"work_order":order})).await["requirements"][0]["issued"],"1.5");
                let undo_issue=apply("reverse_inventory_movement","reverse-issue",issue_reversal.clone()).await;
                assert_eq!(undo_issue["operation_reversal"]["quantity"],"1.5");
                assert_eq!(write("reverse_inventory_movement","duplicate-issue",issue_reversal).await["code"],"MOVEMENT_ALREADY_REVERSED");
                assert_eq!(balance().await["balance"]["on_hand"],"2");
                assert!(invoke("get_material_requirements",json!({"work_order":order})).await["requirements"][0]["issued"].is_null());
                assert_eq!(write("post_operation_report","after-correction",report).await["code"],"MATERIAL_ISSUE_NOT_FOUND");
                assert_eq!(invoke("list_inventory_movements",json!({"movement_type":"issue"})).await["movements"],json!([first["reservation"]]));
                assert_eq!(invoke("list_inventory_movements",json!({"movement_type":"return"})).await["movements"],json!([returned["movement"]]));
                let (peer,_child)=connect_mode(&db,"0",true).await;
                let list=business(&peer,run,"list_inventory_movements",json!({"movement_type":"operation_reversal","location":"RAW-02","limit":1})).await;
                assert_eq!(list["movements"],json!([undo_issue["operation_reversal"]]));
                assert_eq!(list["next_after_movement_id"],"REV-ISSUE");
                let next=invoke("list_inventory_movements",json!({"movement_type":"operation_reversal","after_movement_id":"REV-ISSUE"})).await;
                assert_eq!(next["movements"],json!([undo_return["operation_reversal"]]));
                continue;
            }
            if version >= 28 {
                let descriptor = call(&client, "describe_tool", json!({"run_id":run,"name":"post_operation_report"})).await;
                assert_eq!(descriptor["tool"]["input"]["properties"]["material_inputs"]["type"], "object", "{descriptor}");
                assert_eq!(write("cancel_work_order","cancel",json!({"work_order":order,"reason":"cancel"})).await["code"],"ORDER_HAS_EXECUTION");
                apply("start_operation","start",json!({"work_order":order,"operation":operation})).await;
                let mut report = json!({"work_order":order,"operation":operation,"quantity":1});
                let before = cursor().await;
                assert_eq!(write("post_operation_report","missing",report.clone()).await["code"],"MATERIAL_INPUTS_REQUIRED");
                report["material_inputs"] = json!({"other-run":"0.5"});
                assert_eq!(write("post_operation_report","unissued",report.clone()).await["code"],"MATERIAL_ISSUE_NOT_FOUND");
                assert_eq!(cursor().await,before);
                report["material_inputs"] = json!({"RS-1":"1.125"});
                report["idempotency_key"] = json!("partial");
                report["expected_cursor"] = before;
                let first_report = invoke("post_operation_report",report.clone()).await;
                assert_eq!(first_report["ok"],true,"{first_report}");
                assert_eq!(first_report["report"]["material_inputs"],json!([{
                    "reservation_id":"RS-1","lot_id":"LOT-1","quantity":"1.125","unit":"m"
                }]));
                assert_eq!(first_report["remaining"],9);
                assert_eq!(invoke("post_operation_report",report.clone()).await,first_report);
                report["material_inputs"] = json!({"RS-1":"0.376"});
                let after = cursor().await;
                assert_eq!(write("post_operation_report","excess",report.clone()).await["code"],"MATERIAL_ISSUE_QUANTITY_EXCEEDED");
                assert_eq!(cursor().await,after);
                report["material_inputs"] = json!({"RS-1":"0.375"});
                let second = apply("post_operation_report","second",report.clone()).await;
                assert_eq!(second["reported_quantity"],2);
                apply("reverse_operation_report","reverse",json!({"work_order":order,"report_id":second["report"]["id"],"reason":"correct input"})).await;
                apply("post_operation_report","corrected",report).await;
                assert_eq!(balance().await["balance"]["on_hand"],"0.5");
                let reports=invoke("list_operation_reports",json!({"work_order":order,"operation":operation})).await;
                assert_eq!(reports["reports"].as_array().unwrap().len(),3);
                assert_eq!(reports["reports"][1]["reversed"],true);
                assert_eq!(reports["reports"][1]["material_inputs"],second["report"]["material_inputs"]);
                if version == 29 {
                    let search = call(&client,"search_tools",json!({"run_id":run})).await;
                    assert!(search["tools"].as_array().unwrap().iter().any(|tool|tool["name"]=="return_unused_material"));
                    let mut returned=json!({"movement_id":"RET-1","reservation_id":"RS-1","quantity":"0.25","reason":"unused"});
                    let before=cursor().await;
                    assert_eq!(write("return_unused_material","consumed",returned.clone()).await["code"],"RETURN_EXCEEDS_UNUSED_QUANTITY");
                    assert_eq!(cursor().await,before);
                    apply("reverse_operation_report","reverse-corrected",json!({"work_order":order,"report_id":reports["reports"][2]["id"],"reason":"correct input"})).await;
                    returned["expected_cursor"]=cursor().await;
                    returned["idempotency_key"]=json!("return");
                    let receipt=invoke("return_unused_material",returned.clone()).await;
                    assert_eq!(receipt["ok"],true,"{receipt}");
                    assert_eq!(receipt["movement"]["location"],"RAW-02");
                    assert_eq!(receipt["movement"]["lot_id"],"LOT-1");
                    assert_eq!(invoke("return_unused_material",returned.clone()).await,receipt);
                    assert_eq!(write("return_unused_material","duplicate-movement",returned).await["code"],"RETURN_MOVEMENT_ALREADY_EXISTS");
                    assert_eq!(write("post_operation_report","returned-input",json!({"work_order":order,"operation":operation,"quantity":1,"material_inputs":{"RS-1":"0.126"}})).await["code"],"MATERIAL_ISSUE_QUANTITY_EXCEEDED");
                    apply("return_unused_material","return-rest",json!({"movement_id":"RET-2","reservation_id":"RS-1","quantity":"0.125","reason":"unused"})).await;
                    assert_eq!(balance().await["balance"]["on_hand"],"0.875");
                    let requirements=invoke("get_material_requirements",json!({"work_order":order})).await;
                    assert_eq!(requirements["requirements"][0]["issued"],"1.125");
                    let (peer,_child)=connect_mode(&db,"0",true).await;
                    let page=business(&peer,run,"list_inventory_movements",json!({"movement_type":"return","limit":1,"location":"RAW-02"})).await;
                    assert_eq!(page["movements"],json!([receipt["movement"]]));
                    assert_eq!(page["next_after_movement_id"],"RET-1");
                    let next=invoke("list_inventory_movements",json!({"movement_type":"return","after_movement_id":"RET-1"})).await;
                    assert_eq!(next["movements"][0]["quantity"],"0.125");
                    assert!(next["next_after_movement_id"].is_null());
                    apply("reserve_material","reserve-again",json!({"reservation_id":"RS-4","work_order":order,"lot_id":"LOT-1","quantity":"0.875"})).await;
                    assert_eq!(balance().await["balance"]["available"],"0");
                    let issued=invoke("list_inventory_movements",json!({"movement_type":"issue"})).await;
                    assert_eq!(issued["movements"],json!([first["reservation"]]));
                    let held=invoke("hold_material",json!({"batch":"LOT-1","reason":"segregate lot","idempotency_key":"hold-return"})).await;
                    assert_eq!(held["ok"],true,"{held}");
                    let at_hold=cursor().await;
                    assert_eq!(write("return_unused_material","held-return",json!({"movement_id":"RET-3","reservation_id":"RS-1","quantity":"0.001","reason":"held"})).await["code"],"MATERIAL_HELD");
                    assert_eq!(cursor().await,at_hold);

                    continue;
                }

            }
            let counts=invoke("get_material_requirements",json!({"work_order":order})).await;
            assert_eq!(counts["requirements"][0]["issued"],"1.5");
            assert_eq!(counts["requirements"][0]["shortage"],if scenario=="cnc" {"18"} else {"0"});
            let mut unavailable=reserve;
            unavailable["reservation_id"]=json!("RS-2");
            unavailable["quantity"]=json!("0.6");
            assert_eq!(write("reserve_material","after-issue",unavailable).await["code"],
                if scenario=="cnc" {"LOT_AVAILABLE_QUANTITY_EXCEEDED"} else {"ORDER_REQUIREMENT_EXCEEDED"});
            apply("reserve_material","rs3",json!({"reservation_id":"RS-3","work_order":order,"lot_id":"LOT-1","quantity":"0.5"})).await;
            apply("pick_material","pick3",json!({"reservation_id":"RS-3"})).await;
            apply("create_inspection","qc",json!({"inspection_id":"QC-1","work_order":order,"specification_version":"QA@1"})).await;
            apply("record_measurement","out-of-range",json!({"inspection_id":"QC-1","characteristic":"width",
                "unit":"mm","value":3,"lower_limit":1,"upper_limit":2})).await;
            apply("complete_inspection","complete-qc",json!({"inspection_id":"QC-1"})).await;
            let blocked_at=cursor().await;
            assert_eq!(write("issue_material_to_operation","blocked",json!({"reservation_id":"RS-3","operation":operation})).await["code"],"QUALITY_HOLD");
            assert_eq!(cursor().await,blocked_at);
            assert_eq!(balance().await["balance"]["on_hand"],"0.5");
            let (peer,_child)=connect_mode(&db,"0",true).await;
            let journal=business(&peer,run,"list_inventory_movements",json!({"movement_type":"issue","location":"RAW-02"})).await;
            assert_eq!(journal["movements"],json!([first["reservation"]]));
            assert_eq!(invoke("list_inventory_movements",json!({})).await["movements"][0]["quantity"],"2");
        }
    })).await.unwrap();
}
