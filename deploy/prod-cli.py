#!/usr/bin/env python3
"""Open an interactive shell in the Ultrafinance image on private Fargate."""

import argparse
import getpass
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time
from urllib.parse import parse_qs, urlsplit
import uuid


ROOT = Path(__file__).resolve().parents[1]
DIGEST = re.compile(r"^[0-9]{12}\.dkr\.ecr\.[a-z0-9-]+\.amazonaws\.com/[a-z0-9/_-]+@sha256:[a-f0-9]{64}$")


def output(name):
    result = subprocess.run(
        [str(ROOT / "infra/tofu.sh"), "output", "-json", name],
        capture_output=True, text=True, check=False,
    )
    if result.returncode:
        raise RuntimeError(f"Missing infrastructure output {name}; apply the CLI infrastructure first.")
    return json.loads(result.stdout)


class AWS:
    def __init__(self, profile, region):
        self.prefix = ["aws", "--profile", profile, "--region", region]

    def call(self, *args):
        result = subprocess.run(
            [*self.prefix, *args, "--output", "json"],
            env={**os.environ, "AWS_PAGER": ""},
            capture_output=True, text=True, check=False,
        )
        if result.returncode:
            # Never echo requests, credentials, secret values, or full task definitions.
            if any(word in result.stderr.lower() for word in ("expired", "sso", "login", "credential")):
                raise RuntimeError(f"AWS authentication failed; log in to profile {self.prefix[2]} and retry.")
            match = re.search(r"An error occurred \(([^)]+)\)", result.stderr)
            code = match.group(1) if match else "AWS CLI failed"
            raise RuntimeError(f"{args[0]} {args[1]}: {code}")
        return json.loads(result.stdout) if result.stdout.strip() else {}


def registration(definition, image):
    if not DIGEST.fullmatch(image):
        raise ValueError("Use an immutable ECR image digest, not a tag.")
    fields = (
        "family", "taskRoleArn", "executionRoleArn", "networkMode", "containerDefinitions",
        "volumes", "requiresCompatibilities", "cpu", "memory", "runtimePlatform", "ephemeralStorage",
    )
    request = {key: definition[key] for key in fields if key in definition}
    containers = request["containerDefinitions"]
    cli = next(container for container in containers if container["name"] == "cli")
    cli["image"] = image
    return request


def configure_secret(aws, secret):
    value = getpass.getpass("Production import-role PostgreSQL URL (hidden): ")
    parsed = urlsplit(value)
    if parsed.scheme not in ("postgres", "postgresql") or not parsed.hostname or parse_qs(parsed.query).get("sslmode") not in (["require"], ["verify-full"]):
        raise ValueError("Supply a PostgreSQL URL with sslmode=require or sslmode=verify-full.")
    with tempfile.TemporaryDirectory(prefix="ultrafinance-secret-") as directory:
        path = Path(directory) / "url"
        with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as handle:
            handle.write(value)
        aws.call("secretsmanager", "put-secret-value", "--secret-id", secret, "--secret-string", f"file://{path}")
    print("Import-role URL stored in Secrets Manager.")


def run_shell(aws, config, image=None, timeout=300):
    job_id = uuid.uuid4().hex
    task = None
    revision = None
    launch_attempted = False
    try:
        secret = aws.call("secretsmanager", "describe-secret", "--secret-id", config["database_secret"])
        has_database = any("AWSCURRENT" in stages for stages in secret.get("VersionIdsToStages", {}).values())
        definition = aws.call("ecs", "describe-task-definition", "--task-definition", config["task_definition"])["taskDefinition"]
        chosen_image = image or aws.call(
            "lambda", "get-function", "--function-name", config["function_name"],
            "--qualifier", "live", "--query", "Code.ResolvedImageUri",
        )
        request = registration(definition, chosen_image)
        container = next(item for item in request["containerDefinitions"] if item["name"] == "cli")
        # An empty import secret must not prevent opening a shell.
        container["secrets"] = ([{"name": "ULTRAFINANCE_DATABASE_URL", "valueFrom": config["database_secret"]}]
                                if has_database else [])
        with tempfile.TemporaryDirectory(prefix="ultrafinance-shell-") as directory:
            path = Path(directory) / "task.json"
            path.write_text(json.dumps(request))
            revision = aws.call("ecs", "register-task-definition", "--cli-input-json", f"file://{path}")["taskDefinition"]["taskDefinitionArn"]
        network = {"awsvpcConfiguration": {
            "subnets": config["subnets"], "securityGroups": config["security_groups"], "assignPublicIp": "DISABLED",
        }}
        print(f"Session: {job_id}", flush=True)
        launch_attempted = True
        try:
            result = aws.call("ecs", "run-task", "--cluster", config["cluster"],
                              "--task-definition", revision, "--launch-type", "FARGATE",
                              "--platform-version", "1.4.0", "--count", "1", "--client-token", job_id,
                              "--started-by", job_id, "--enable-execute-command",
                              "--network-configuration", json.dumps(network))
        except RuntimeError as error:
            raise RuntimeError(f"{error}. Launch outcome is uncertain; check ECS tasks started by {job_id}.") from error
        tasks = result.get("tasks", [])
        if not tasks:
            launch_attempted = False
            reasons = ", ".join(failure.get("reason", "unknown") for failure in result.get("failures", []))
            raise RuntimeError(f"Fargate did not start a task: {reasons}")
        task = tasks[0]["taskArn"]
        print(f"Task: {task}", flush=True)
        print(f"Image: {chosen_image}", flush=True)
        if not has_database:
            print("Database URL is not configured; use --configure-database before a future session to enable production database access.", flush=True)
        deadline = time.monotonic() + timeout
        while True:
            response = aws.call("ecs", "describe-tasks", "--cluster", config["cluster"], "--tasks", task)
            if response.get("failures") or not response.get("tasks"):
                raise RuntimeError(f"Unable to inspect task {task}.")
            status = response["tasks"][0]
            if status["lastStatus"] == "STOPPED":
                raise RuntimeError(f"Shell task stopped before connecting ({status.get('stopCode', 'unknown')}); inspect ECS and {config['log_group']}.")
            ready = any(
                agent.get("name") == "ExecuteCommandAgent" and agent.get("lastStatus") == "RUNNING"
                for item in status.get("containers", []) if item["name"] == "cli"
                for agent in item.get("managedAgents", [])
            )
            if status["lastStatus"] == "RUNNING" and ready:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError("Timed out waiting for the ECS Exec agent.")
            time.sleep(5)
        print("Opening shell. Run ultrafinance commands here; exiting stops the task. The task expires after one hour.", flush=True)
        result = subprocess.run(
            [*aws.prefix, "ecs", "execute-command", "--cluster", config["cluster"],
             "--task", task, "--container", "cli", "--interactive", "--command", "/bin/sh"],
            env={**os.environ, "AWS_PAGER": ""}, check=False,
        )
        if result.returncode:
            raise RuntimeError("ECS Exec session failed.")
    finally:
        # Known tasks are stopped on exit, startup failure, timeout, or Ctrl-C.
        # An uncertain launch is bounded by the task's one-hour lifetime.
        if task:
            aws.call("ecs", "stop-task", "--cluster", config["cluster"], "--task", task,
                     "--reason", "Interactive shell session ended")
        if revision and (task or not launch_attempted):
            aws.call("ecs", "deregister-task-definition", "--task-definition", revision)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", help="Immutable ECR digest; defaults to Lambda's live image")
    parser.add_argument("--configure-database", action="store_true", help="Securely store the import-role URL for shell sessions")
    args = parser.parse_args()
    if not args.configure_database:
        if not sys.stdin.isatty() or not sys.stdout.isatty():
            parser.error("Run this command in an interactive terminal.")
        if not shutil.which("session-manager-plugin"):
            parser.error("Install the AWS Session Manager plugin before opening a shell.")
    config = output("cli_runner")
    if not config:
        raise RuntimeError("Apply the Aurora and application infrastructure first; CLI tasks are provisioned automatically.")
    aws = AWS(output("aws_profile"), output("aws_region"))
    if args.configure_database:
        configure_secret(aws, config["database_secret"])
    else:
        run_shell(aws, config, args.image)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, ValueError, OSError) as error:
        print(f"Error: {error}", file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        print("Shell interrupted; a known task is stopped automatically.", file=sys.stderr)
        sys.exit(130)
