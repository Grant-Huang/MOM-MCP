// Included in the pinned DES repository's stdio integration tests by check-agentnexus.sh.
#[tokio::test]
async fn p0_des_two_seed_replay() {
    let mut traces = Vec::new();
    for seed in [1, 17] {
        let mut steps = script();
        steps[0].1["scenario"]["seed"] = seed.into();
        let mut expected = Vec::new();
        let mut local = FactoryService::new(ServiceConfig {
            admin_token: Some(ADMIN.into()),
        });
        for (name, args) in &steps {
            expected.push(local.call(name, args.clone()).expect(name));
        }
        let server = start().await;
        for ((name, args), want) in steps.iter().zip(&expected) {
            assert_eq!(
                &call(&server, name, args.clone()).await,
                want,
                "{name}, seed {seed}"
            );
        }
        let mut trace = Vec::new();
        let server = start().await;
        for ((name, args), want) in steps.into_iter().zip(expected) {
            assert_eq!(
                call(&server, name, args.clone()).await,
                want,
                "{name}, seed {seed}"
            );
            let mut inputs = args;
            if let Some(fields) = inputs.as_object_mut() {
                fields.remove("admin_token");
            }
            trace.push(json!({"tool": name, "arguments": inputs, "result": want}));
        }
        traces.push(json!({"seed": seed, "trace": trace}));
    }
    if let Some(path) = std::env::var_os("MOM_DES_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&traces).unwrap()).unwrap();
    }
}
