use super::*;
#[test]
fn tunnel_targets_cli_runtime_and_uses_task_id_not_arn() {
    let response = json!({"tasks":[{"containers":[{"name":"other","runtimeId":"wrong"},{"name":"cli","runtimeId":"runtime"}]}]});
    assert_eq!(
        target(
            "arn:aws:ecs:region:account:cluster/cluster",
            "arn:aws:ecs:region:account:task/cluster/task",
            &response
        )
        .unwrap(),
        "ecs:cluster_task_runtime"
    );
    assert!(target("cluster", "task", &json!({})).is_err());
}
#[test]
fn restore_is_atomic_local_and_strips_production_ownership() {
    let command = restore_command(Path::new("snapshot.dump"));
    let args: Vec<_> = command
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(args.contains(&LOCAL_DATABASE_URL.to_owned()));
    for option in [
        "--clean",
        "--if-exists",
        "--single-transaction",
        "--exit-on-error",
        "--no-owner",
        "--no-privileges",
    ] {
        assert!(args.iter().any(|a| a == option));
    }
    assert!(
        command
            .get_envs()
            .any(|(k, v)| k == "PGPASSWORD" && v.is_none())
    );
}
#[test]
fn connection_config_only_comes_from_cli_container() {
    let definition = json!({"taskDefinition":{"containerDefinitions":[{"name":"cli","environment":[{"name":"ULTRAFINANCE_DATABASE_NAME","value":"finance"}]}]}});
    assert_eq!(
        database_environment(&definition, "ULTRAFINANCE_DATABASE_NAME").unwrap(),
        "finance"
    );
    assert!(database_environment(&definition, "ULTRAFINANCE_DATABASE_HOST").is_err());
}
