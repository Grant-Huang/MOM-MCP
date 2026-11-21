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
    let request = CallToolRequestParams::new(name.to_owned())
        .with_arguments(args.as_object().unwrap().clone());
    let response = client.peer().call_tool(request).await.unwrap();
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
        json!({"scenario_id":scenario,"version":1,"seed":42}),
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
        assert_eq!(rejected["code"], "MODEL_CONFIGURATION_INVALID");
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
        json!({"scenario_id":"cnc","version":2,"seed":42}),
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
        json!({"scenario_id":"smt","version":2,"seed":42}),
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
    timeout(WAIT, async {
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
    .unwrap();
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
        json!({"scenario_id":id,"version":version,"seed":42}),
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
