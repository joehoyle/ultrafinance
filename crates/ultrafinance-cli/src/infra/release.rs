use super::aws::{Api, Aws, text, validate_image};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

trait Lambda {
    fn call(&self, operation: &str, arguments: &[&str]) -> Result<Value>;
    fn check(&self, qualifier: &str) -> Result<()>;
}

struct Client<'a> {
    aws: &'a Aws,
    function: &'a str,
}

impl Lambda for Client<'_> {
    fn call(&self, operation: &str, arguments: &[&str]) -> Result<Value> {
        let mut args = vec!["lambda", operation];
        // Waiter name precedes options in the AWS CLI.
        args.extend_from_slice(arguments);
        args.extend(["--function-name", self.function]);
        self.aws.call(&args)
    }

    fn check(&self, qualifier: &str) -> Result<()> {
        for (method, path, body, expected) in [
            ("GET", "/health", Value::Null, 200),
            ("GET", "/", Value::Null, 200),
            ("GET", "/docs", Value::Null, 200),
            ("GET", "/openapi.json", Value::Null, 200),
            ("GET", "/v1/merchants", Value::Null, 200),
            ("POST", "/v1/enrich", json!("{\"description\":\"\"}"), 422),
        ] {
            let directory = tempfile::tempdir()?;
            let payload = directory.path().join("event.json");
            let response = directory.path().join("response.json");
            let event = json!({
                "version":"2.0", "routeKey":"$default", "rawPath":path,
                "rawQueryString":"", "headers":{"content-type":"application/json", "host":"localhost"},
                "requestContext":{"routeKey":"$default", "stage":"$default", "requestId":"deploy-check",
                    "timeEpoch":0, "domainName":"localhost", "accountId":"deploy-check",
                    "http":{"method":method,"path":path,"protocol":"HTTP/1.1","sourceIp":"127.0.0.1","userAgent":"deploy-check"}},
                "body":body, "isBase64Encoded":false
            });
            std::fs::write(&payload, serde_json::to_vec(&event)?)?;
            let metadata = self.call(
                "invoke",
                &[
                    "--qualifier",
                    qualifier,
                    "--payload",
                    &format!("fileb://{}", payload.display()),
                    response.to_str().context("invalid temporary path")?,
                ],
            )?;
            if metadata.get("FunctionError").is_some() {
                bail!("Lambda {qualifier} failed its {path} check");
            }
            let result: Value = serde_json::from_slice(&std::fs::read(response)?)?;
            if result["statusCode"] != expected {
                bail!(
                    "Lambda {qualifier} returned {} for {path}, expected {expected}",
                    result["statusCode"]
                );
            }
            let body = text(&result, "body")?;
            let decoded = if result["isBase64Encoded"] == true {
                STANDARD.decode(body)?
            } else {
                body.as_bytes().to_vec()
            };
            verify_body(path, &decoded)?;
        }
        Ok(())
    }
}

fn verify_body(path: &str, body: &[u8]) -> Result<()> {
    if path == "/health" {
        let health: Value = serde_json::from_slice(body)?;
        if health["status"] != "ok" {
            bail!("health check failed");
        }
    }
    if path == "/openapi.json" {
        let spec: Value = serde_json::from_slice(body)?;
        for (path, method) in [
            ("/health", "get"),
            ("/v1/merchants", "get"),
            ("/v1/enrich", "post"),
            ("/v1/enrich/batch", "post"),
        ] {
            if !spec["paths"][path][method].is_object() {
                bail!("OpenAPI missing {method} {path}");
            }
        }
    }
    Ok(())
}

fn release(client: &impl Lambda, image: &str) -> Result<(String, String)> {
    let previous = client.call("get-alias", &["--name", "live"])?;
    let current = client.call("get-function-configuration", &[])?;
    client.call(
        "update-function-code",
        &[
            "--image-uri",
            image,
            "--revision-id",
            text(&current, "RevisionId")?,
        ],
    )?;
    client.call("wait", &["function-updated-v2"])?;
    let updated = client.call("get-function", &[])?;
    if updated["Code"]["ResolvedImageUri"] != image {
        bail!("function image changed during deployment; live was not promoted");
    }
    let config = &updated["Configuration"];
    let versions = client.call("list-versions-by-function", &[])?;
    let fields = if config["ConfigSha256"]
        .as_str()
        .is_some_and(|s| !s.is_empty())
    {
        vec!["CodeSha256", "ConfigSha256"]
    } else {
        vec![
            "CodeSha256",
            "Role",
            "Runtime",
            "Handler",
            "Description",
            "Timeout",
            "MemorySize",
            "Environment",
            "VpcConfig",
            "DeadLetterConfig",
            "KMSKeyArn",
            "TracingConfig",
            "Layers",
            "FileSystemConfigs",
            "ImageConfigResponse",
            "Architectures",
            "EphemeralStorage",
            "SnapStart",
            "LoggingConfig",
        ]
    };
    let existing = versions["Versions"]
        .as_array()
        .context("AWS response missing Versions")?
        .iter()
        .filter(|item| {
            fields
                .iter()
                .all(|field| item.get(*field) == config.get(*field))
        })
        .filter_map(|item| item["Version"].as_str()?.parse::<u64>().ok())
        .max();
    let version = if let Some(version) = existing {
        version.to_string()
    } else {
        let published = client.call(
            "publish-version",
            &[
                "--revision-id",
                text(config, "RevisionId")?,
                "--description",
                config["Description"].as_str().unwrap_or(""),
            ],
        )?;
        text(&published, "Version")?.to_owned()
    };
    client.check(&version)?;
    let promoted = client.call(
        "update-alias",
        &[
            "--name",
            "live",
            "--function-version",
            &version,
            "--revision-id",
            text(&previous, "RevisionId")?,
            "--routing-config",
            "{\"AdditionalVersionWeights\":{}}",
        ],
    )?;
    if let Err(error) = client.check("live") {
        let routing = previous
            .get("RoutingConfig")
            .cloned()
            .unwrap_or_else(|| json!({"AdditionalVersionWeights":{}}))
            .to_string();
        client
            .call(
                "update-alias",
                &[
                    "--name",
                    "live",
                    "--function-version",
                    text(&previous, "FunctionVersion")?,
                    "--revision-id",
                    text(&promoted, "RevisionId")?,
                    "--routing-config",
                    &routing,
                ],
            )
            .context("live check failed and guarded rollback failed; inspect the live alias")?;
        return Err(error.context("live check failed; previous version restored"));
    }
    Ok((version, text(&previous, "FunctionVersion")?.to_owned()))
}

pub fn deploy(root: &Path, image: Option<&str>) -> Result<()> {
    let aws = Aws::configured(root)?;
    let function = super::output(root, "function_name")?;
    let image = if let Some(image) = image {
        validate_image(image)?;
        image.to_owned()
    } else {
        build(root, &aws)?
    };
    promote(root, &aws, &function, &image)
}

fn build(root: &Path, aws: &Aws) -> Result<String> {
    let repository = super::output(root, "repository_url")?;
    let (registry, name) = repository
        .split_once('/')
        .context("invalid ECR repository URL")?;
    let password = aws.command().args(["ecr", "get-login-password"]).output()?;
    if !password.status.success() {
        bail!(
            "ECR authentication failed; log in to profile {} and retry",
            aws.profile
        );
    }
    let mut login = Command::new("docker")
        .args(["login", "--username", "AWS", "--password-stdin", registry])
        .stdin(Stdio::piped())
        .spawn()
        .context("could not run Docker")?;
    let write = login
        .stdin
        .take()
        .context("Docker login stdin unavailable")?
        .write_all(&password.stdout);
    let status = login.wait()?;
    write.context("could not send ECR login to Docker")?;
    if !status.success() {
        bail!("Docker ECR login failed");
    }
    let tag = format!("{repository}:{}", uuid::Uuid::new_v4().simple());
    // The Dockerfile runs workspace tests and Clippy before pushing the final image.
    let metadata = crate::build_metadata::BuildMetadata::capture(root);
    eprintln!(
        "Building CLI version {}",
        metadata.version(env!("CARGO_PKG_VERSION"))
    );
    let mut command = Command::new("docker");
    command.current_dir(root).args([
        "buildx",
        "build",
        "--platform",
        "linux/arm64",
        "--provenance=false",
        "--tag",
        &tag,
        "--push",
    ]);
    for (name, value) in [
        (
            "ULTRAFINANCE_BUILD_TAG",
            metadata.tag.as_deref().unwrap_or(""),
        ),
        (
            "ULTRAFINANCE_BUILD_REVISION",
            metadata.revision.as_deref().unwrap_or(""),
        ),
        (
            "ULTRAFINANCE_BUILD_DIRTY",
            if metadata.dirty { "true" } else { "false" },
        ),
        ("ULTRAFINANCE_BUILD_TIME", metadata.time.as_str()),
    ] {
        command.args(["--build-arg", &format!("{name}={value}")]);
    }
    super::checked(command.arg("."))?;
    let image_tag = format!(
        "imageTag={}",
        tag.rsplit(':').next().context("missing release tag")?
    );
    let result = aws.call(&[
        "ecr",
        "describe-images",
        "--repository-name",
        name,
        "--image-ids",
        &image_tag,
        "--query",
        "imageDetails[0].imageDigest",
    ])?;
    let digest = result.as_str().context("ECR image digest missing")?;
    let image = format!("{repository}@{digest}");
    validate_image(&image)?;
    Ok(image)
}

fn promote(root: &Path, aws: &Aws, function: &str, image: &str) -> Result<()> {
    let (version, previous) = release(&Client { aws, function }, image)?;
    println!("Deployed version {version}; previous version {previous}");
    let site = super::output(root, "site_url")?;
    for path in ["/health", "/docs", "/openapi.json"] {
        let response = Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--max-time",
                "70",
                &format!("{}{path}", site.trim_end_matches('/')),
            ])
            .output()
            .context("could not run curl for public checks")?;
        if !response.status.success() {
            bail!("version {version} is live, but public {path} verification failed");
        }
        verify_body(path, &response.stdout).with_context(|| {
            format!("version {version} is live, but public {path} verification failed")
        })?;
        println!("Verified {path}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
            if self.fail_check == Some(qualifier) {
                bail!("smoke check failed");
            }
            Ok(())
        }
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
}
