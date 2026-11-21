use std::{path::Path, process::Stdio, time::Duration};

use rmcp::{RoleClient, ServiceExt, model::CallToolRequestParams, service::RunningService};
use serde_json::{Value, json};
use tokio::{
    process::{Child, Command},
    time::timeout,
};

const WAIT: Duration = Duration::from_secs(5);

async fn connect(db: &Path, delay: &str) -> (RunningService<RoleClient, ()>, Child) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mom-mcp"))
        .arg("stdio")
        .arg("--db")
        .arg(db)
        .args(["--response-delay-ms", delay])
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
