#!/usr/bin/env python3
"""Run the existing Ultrafinance binary as a one-off private Fargate task."""

import argparse
import getpass
import json
import os
from pathlib import Path
import re
import sqlite3
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


def validate_command(command, has_input):
    if not command:
        raise ValueError("Supply a CLI command after --.")
    if any(arg.startswith("--database-url") or "postgres://" in arg or "postgresql://" in arg for arg in command):
        raise ValueError("Database credentials must come from the import secret, not command arguments.")
    uses_input = "@input" in command
    if uses_input != has_input:
        raise ValueError("Use --input FILE and an @input command argument together.")
    # This runner supports shared-catalog operations, not local-only files or provider credentials.
    if command[0] not in ("merchants", "database", "datasets"):
        raise ValueError("Production runner supports merchants, database, and datasets apply commands.")
    if command[0] == "datasets" and command[1:2] != ["apply"]:
        raise ValueError("Prepare datasets locally, then run datasets apply in production.")
    return ["/input/source" if arg == "@input" else arg for arg in command]


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


def snapshot_sqlite(source, destination):
    # SQLite backup includes committed WAL contents without altering the local catalog.
    connection = sqlite3.connect(source.resolve().as_uri() + "?mode=ro", uri=True)
    try:
        with sqlite3.connect(destination) as backup:
            connection.backup(backup)
    finally:
        connection.close()


def print_logs(aws, config, task):
    task_id = task.rsplit("/", 1)[-1]
    for container in ("input", "cli"):
        token = None
        while True:
            args = ["logs", "filter-log-events", "--log-group-name", config["log_group"],
                    "--log-stream-names", f"cli/{container}/{task_id}"]
            if token:
                args.extend(["--next-token", token])
            response = aws.call(*args)
            for event in response.get("events", []):
                print(event["message"])
            next_token = response.get("nextToken")
            if not next_token or next_token == token:
                break
            token = next_token


def run_job(aws, config, command, source, image, timeout):
    job_id = uuid.uuid4().hex
    task = None
    staged_key = None
    revision = None
    stopped = False
    launch_attempted = False
    try:
        # Validate the secret exists, without retrieving its value to this machine.
        secret = aws.call("secretsmanager", "describe-secret", "--secret-id", config["database_secret"])
        if not any("AWSCURRENT" in stages for stages in secret.get("VersionIdsToStages", {}).values()):
            raise RuntimeError("Import secret has no value; run --configure-database first.")
        definition = aws.call("ecs", "describe-task-definition", "--task-definition", config["task_definition"])["taskDefinition"]
        cli = next(container for container in definition["containerDefinitions"] if container["name"] == "cli")
        chosen_image = image or aws.call(
            "lambda", "get-function", "--function-name", config["function_name"],
            "--qualifier", "live", "--query", "Code.ResolvedImageUri",
        )
        if not DIGEST.fullmatch(chosen_image):
            raise ValueError("The task must use an immutable ECR image digest.")
        task_definition = config["task_definition"]
        if chosen_image != cli["image"]:
            with tempfile.TemporaryDirectory(prefix="ultrafinance-task-") as directory:
                path = Path(directory) / "task.json"
                path.write_text(json.dumps(registration(definition, chosen_image)))
                revision = aws.call("ecs", "register-task-definition", "--cli-input-json", f"file://{path}")["taskDefinition"]["taskDefinitionArn"]
            task_definition = revision
        if source:
            staged_key = f"jobs/{job_id}/source"
            with tempfile.TemporaryDirectory(prefix="ultrafinance-input-") as directory:
                upload = source
                if command[:2] == ["database", "migrate-sqlite"]:
                    upload = Path(directory) / "catalog.sqlite"
                    snapshot_sqlite(source, upload)
                aws.call("s3", "cp", str(upload), f"s3://{config['input_bucket']}/{staged_key}", "--only-show-errors", "--sse", "AES256")
        overrides = {"containerOverrides": [
            {"name": "cli", "command": command},
            {"name": "input", "environment": [{"name": "INPUT_KEY", "value": staged_key or ""}]},
        ]}
        network = {"awsvpcConfiguration": {
            "subnets": config["subnets"], "securityGroups": config["security_groups"], "assignPublicIp": "DISABLED",
        }}
        print(f"Job: {job_id}", flush=True)
        launch_attempted = True
        try:
            result = aws.call("ecs", "run-task", "--cluster", config["cluster"],
                              "--task-definition", task_definition, "--launch-type", "FARGATE",
                              "--platform-version", "1.4.0", "--count", "1", "--client-token", job_id,
                              "--started-by", job_id,
                              "--network-configuration", json.dumps(network), "--overrides", json.dumps(overrides))
        except RuntimeError as error:
            raise RuntimeError(f"{error}. Launch outcome is uncertain; check ECS tasks started by {job_id} before retrying.") from error
        tasks = result.get("tasks", [])
        if not tasks:
            launch_attempted = False
            reasons = ", ".join(failure.get("reason", "unknown") for failure in result.get("failures", []))
            raise RuntimeError(f"Fargate did not start a task: {reasons}")
        task = tasks[0]["taskArn"]
        print(f"Task: {task}", flush=True)
        print(f"Image: {chosen_image}", flush=True)
        print(f"Logs: {config['log_group']}", flush=True)
        deadline = time.monotonic() + timeout
        while True:
            response = aws.call("ecs", "describe-tasks", "--cluster", config["cluster"], "--tasks", task)
            if response.get("failures") or not response.get("tasks"):
                raise RuntimeError(f"Unable to inspect task {task}; check ECS before retrying the import.")
            status = response["tasks"][0]
            if status["lastStatus"] == "STOPPED":
                stopped = True
                break
            if time.monotonic() >= deadline:
                raise RuntimeError(f"Wait timed out; task {task} continues running. Check ECS before retrying.")
            time.sleep(5)
        # Allow the final log batch to reach CloudWatch.
        time.sleep(3)
        try:
            print_logs(aws, config, task)
        except RuntimeError as error:
            raise RuntimeError(f"{error}. Task {task} stopped, but logs could not be read; verify its exit status before retrying.") from error
        containers = {container["name"]: container for container in status.get("containers", [])}
        cli_exit = containers.get("cli", {}).get("exitCode")
        input_exit = containers.get("input", {}).get("exitCode")
        if status.get("stopCode") != "EssentialContainerExited" or cli_exit != 0 or input_exit != 0:
            raise RuntimeError(f"Task failed ({status.get('stopCode', 'unknown')}): input exit={input_exit}, CLI exit={cli_exit}. Check the ECS task and logs.")
        print("Production CLI completed successfully.")
    finally:
        # On interrupted/uncertain runs leave resources intact: never retry writes automatically.
        if not launch_attempted or stopped:
            if staged_key:
                aws.call("s3", "rm", f"s3://{config['input_bucket']}/{staged_key}", "--only-show-errors")
            if revision:
                aws.call("ecs", "deregister-task-definition", "--task-definition", revision)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, help="Local input file; use @input in the CLI command")
    parser.add_argument("--image", help="Optional immutable ECR digest; defaults to the image deployed to Lambda's live alias")
    parser.add_argument("--timeout", type=int, default=3600, help="Seconds to wait; a timeout leaves the task running")
    parser.add_argument("--configure-database", action="store_true", help="Securely prompt for and store the import-role URL")
    parser.add_argument("command", nargs=argparse.REMAINDER, help="CLI arguments after --")
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if args.configure_database:
        if command or args.input or args.image:
            parser.error("--configure-database cannot be combined with a job.")
    else:
        command = validate_command(command, args.input is not None)
        if args.input and not args.input.is_file():
            parser.error("Input file does not exist.")
        if args.timeout <= 0:
            parser.error("Timeout must be positive.")
    config = output("cli_runner")
    if not config:
        raise RuntimeError("Apply the Aurora and application infrastructure first; CLI tasks are provisioned automatically.")
    aws = AWS(output("aws_profile"), output("aws_region"))
    if args.configure_database:
        configure_secret(aws, config["database_secret"])
    else:
        run_job(aws, config, command, args.input, args.image, args.timeout)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, ValueError, OSError, sqlite3.Error) as error:
        print(f"Error: {error}", file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        print("Stopped waiting. A started task continues running; check ECS before retrying.", file=sys.stderr)
        sys.exit(130)
