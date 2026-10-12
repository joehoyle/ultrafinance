use super::*;

#[test]
fn oidc_commands_remove_profile_and_local_commands_select_it() {
    let oidc = Aws {
        profile: String::new(),
        region: "ci-region".into(),
    }
    .command();
    assert!(
        oidc.get_envs()
            .any(|(key, value)| key == "AWS_PROFILE" && value.is_none())
    );
    assert_eq!(
        oidc.get_args().collect::<Vec<_>>(),
        ["--region", "ci-region"]
    );
    let local = Aws {
        profile: "local-profile".into(),
        region: "local-region".into(),
    }
    .command();
    assert_eq!(
        local.get_args().collect::<Vec<_>>(),
        ["--profile", "local-profile", "--region", "local-region"]
    );
}
