use super::*;
use std::cell::RefCell;

#[derive(Default)]
struct Fake {
    calls: RefCell<Vec<(String, Vec<String>)>>,
    checks: RefCell<Vec<String>>,
    fail_check: Option<&'static str>,
    alias_conflict: bool,
    image_conflict: bool,
    unchanged: bool,
    catalog_failures: RefCell<usize>,
    schema_mismatch: bool,
}

impl Lambda for Fake {
    fn call(&self, operation: &str, args: &[&str]) -> Result<Value> {
        self.calls.borrow_mut().push((
            operation.into(),
            args.iter().map(|s| s.to_string()).collect(),
        ));
        Ok(match operation {
            "get-alias" => json!({"FunctionVersion":"7","RevisionId":"old-alias"}),
            "get-function-configuration" => json!({"RevisionId":"function-revision"}),
            "get-function" => {
                json!({"Configuration":{"RevisionId":"function-revision","CodeSha256":"same-code","ConfigSha256":"same-config"},
                "Code":{"ResolvedImageUri":if self.image_conflict {"other-image"} else {"image"}}})
            }
            "list-versions-by-function" => json!({"Versions":if self.unchanged {
                vec![json!({"Version":"8","CodeSha256":"same-code","ConfigSha256":"same-config"})]
            } else { vec![] }}),
            "publish-version" => json!({"Version":"8"}),
            "update-alias" => {
                if self.alias_conflict {
                    bail!("alias changed by another deploy");
                }
                json!({"RevisionId":"promoted-alias"})
            }
            _ => json!({}),
        })
    }

    fn check(&self, qualifier: &str) -> Result<()> {
        self.checks.borrow_mut().push(qualifier.into());
        if self.schema_mismatch {
            verify_schema_compatibility(qualifier, br#"{"error":{"code":"database_schema_mismatch","message":"credentials must not be echoed"}}"#)?;
        }
        let mut failures = self.catalog_failures.borrow_mut();
        if *failures > 0 {
            *failures -= 1;
            return Err(CatalogNotReady {
                qualifier: qualifier.into(),
                status: 503,
            }
            .into());
        }
        if self.fail_check == Some(qualifier) {
            bail!("smoke check failed");
        }
        Ok(())
    }
}

#[test]
fn schema_mismatch_never_retries_or_promotes_and_does_not_echo_response_details() {
    let client = Fake {
        schema_mismatch: true,
        ..Default::default()
    };
    let error = release(&client, "image").unwrap_err();
    assert!(error.to_string().contains("schema-upgrade cutover"));
    assert!(!error.to_string().contains("credentials must not be echoed"));
    assert_eq!(*client.checks.borrow(), vec!["8"]);
    assert!(
        !client
            .calls
            .borrow()
            .iter()
            .any(|(operation, _)| operation == "update-alias")
    );
    for body in [
        br#"{"error":{"code":"catalog_unavailable"}}"#.as_slice(),
        b"<html>Unavailable</html>",
    ] {
        assert!(verify_schema_compatibility("8", body).is_ok());
    }
}

#[test]
fn catalog_resume_is_retried_with_a_bounded_budget() {
    for (failures, succeeds, checks) in [(2, true, 3), (3, false, 3)] {
        let client = Fake {
            catalog_failures: RefCell::new(failures),
            ..Default::default()
        };
        let mut pauses = 0;
        let result = check_with_resume_retry(&client, "8", || pauses += 1);
        assert_eq!(result.is_ok(), succeeds);
        assert_eq!(client.checks.borrow().len(), checks);
        assert_eq!(pauses, 2);
    }
}

#[test]
fn other_readiness_failures_are_not_retried() {
    let client = Fake {
        fail_check: Some("8"),
        ..Default::default()
    };
    assert!(check_with_resume_retry(&client, "8", || panic!("unexpected retry")).is_err());
    assert_eq!(client.checks.borrow().len(), 1);
}

#[test]
fn candidate_failure_never_promotes() {
    let client = Fake {
        fail_check: Some("8"),
        ..Default::default()
    };
    assert!(release(&client, "image").is_err());
    assert!(
        !client
            .calls
            .borrow()
            .iter()
            .any(|(op, _)| op == "update-alias")
    );
}

#[test]
fn promotion_and_rollback_use_revision_guards() {
    for fail in [false, true] {
        let client = Fake {
            fail_check: if fail { Some("live") } else { None },
            ..Default::default()
        };
        let result = release(&client, "image");
        assert_eq!(result.is_err(), fail);
        assert_eq!(*client.checks.borrow(), vec!["8", "live"]);
        let calls = client.calls.borrow();
        let changes: Vec<_> = calls
            .iter()
            .filter(|(op, _)| op == "update-alias")
            .collect();
        assert_eq!(changes.len(), if fail { 2 } else { 1 });
        assert!(changes[0].1.iter().any(|s| s == "old-alias"));
        if fail {
            assert!(changes[1].1.iter().any(|s| s == "promoted-alias"));
            assert!(changes[1].1.iter().any(|s| s == "7"));
        }
        assert!(
            calls
                .iter()
                .find(|(op, _)| op == "publish-version")
                .unwrap()
                .1
                .iter()
                .any(|s| s == "function-revision")
        );
    }
}

#[test]
fn racing_changes_are_not_overwritten() {
    let client = Fake {
        alias_conflict: true,
        ..Default::default()
    };
    assert!(release(&client, "image").is_err());
    assert_eq!(
        client
            .calls
            .borrow()
            .iter()
            .filter(|(op, _)| op == "update-alias")
            .count(),
        1
    );
    let client = Fake {
        image_conflict: true,
        ..Default::default()
    };
    assert!(release(&client, "image").is_err());
    assert!(
        !client
            .calls
            .borrow()
            .iter()
            .any(|(op, _)| op == "publish-version" || op == "update-alias")
    );
}

#[test]
fn unchanged_image_reuses_checked_version() {
    let client = Fake {
        unchanged: true,
        ..Default::default()
    };
    assert_eq!(release(&client, "image").unwrap(), ("8".into(), "7".into()));
    assert!(
        !client
            .calls
            .borrow()
            .iter()
            .any(|(op, _)| op == "publish-version")
    );
}

#[test]
fn public_checks_require_healthy_service_and_expected_api_paths() {
    assert!(verify_body("/health", br#"{"status":"bad"}"#).is_err());
    assert!(verify_body("/health", br#"{"status":"ok"}"#).is_ok());
    assert!(verify_body("/openapi.json", br#"{"paths":{}}"#).is_err());
    let spec = json!({"paths":{"/health":{"get":{}}, "/v1/merchants":{"get":{}},
        "/v1/enrich":{"post":{}}, "/v1/enrich/batch":{"post":{}}}});
    assert!(verify_body("/openapi.json", &serde_json::to_vec(&spec).unwrap()).is_ok());
}
