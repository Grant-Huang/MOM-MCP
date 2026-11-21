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
    let mut command = Command::new(env!("CARGO_BIN_EXE_mom-mcp"));
    command
        .args(["stdio", "--db"])
        .arg(db)
        .args(["--response-delay-ms", delay]);
    if scenarios {
        command.arg("--scenario-tools");
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
