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
