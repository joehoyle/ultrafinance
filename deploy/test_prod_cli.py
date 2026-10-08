import contextlib
import importlib.util
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("prod_cli", Path(__file__).with_name("prod-cli.py"))
cli = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cli)

IMAGE = "123456789012.dkr.ecr.ca-central-1.amazonaws.com/ultrafinance@sha256:" + "a" * 64
CONFIG = {"cluster": "cluster", "task_definition": "definition", "database_secret": "secret",
          "subnets": ["private"], "security_groups": ["db-client"], "log_group": "logs", "function_name": "function"}


class FakeAWS:
    prefix = ["aws", "--profile", "test", "--region", "ca-central-1"]
    environment = {"AWS_PAGER": "", "AWS_REGION": "ca-central-1", "AWS_DEFAULT_REGION": "ca-central-1"}

    def __init__(self, has_secret=True, launch_error=False, ready=True, rejected=False):
        self.calls = []
        self.has_secret = has_secret
        self.launch_error = launch_error
        self.ready = ready
        self.rejected = rejected
        self.registered = None

    def call(self, *args):
        self.calls.append(args)
        operation = args[:2]
        if operation == ("ecs", "describe-task-definition"):
            return {"taskDefinition": {"family": "cli", "containerDefinitions": [{"name": "cli", "image": IMAGE,
                "secrets": [{"name": "ULTRAFINANCE_DATABASE_URL", "valueFrom": "secret"}] if self.has_secret else []}]}}
        if operation == ("lambda", "get-function"):
            return IMAGE
        if operation == ("ecs", "register-task-definition"):
            self.registered = json.loads(Path(args[-1].removeprefix("file://")).read_text())
            return {"taskDefinition": {"taskDefinitionArn": "revision"}}
        if operation == ("ecs", "run-task"):
            if self.launch_error:
                raise RuntimeError("uncertain launch")
            if self.rejected:
                return {"tasks": [], "failures": [{"reason": "capacity"}]}
            return {"tasks": [{"taskArn": "task/123"}]}
        if operation == ("ecs", "describe-tasks"):
            return {"tasks": [{"lastStatus": "RUNNING", "containers": [{"name": "cli", "managedAgents": [
                {"name": "ExecuteCommandAgent", "lastStatus": "RUNNING" if self.ready else "PENDING"}
            ]}]}]}
        return {}


class ProductionShellTests(unittest.TestCase):
    def shell(self, aws, exec_code=0, exception=None):
        with patch.object(cli.time, "sleep"), contextlib.redirect_stdout(io.StringIO()):
            with patch.object(cli.subprocess, "run") as execute:
                if exception:
                    execute.side_effect = exception
                else:
                    execute.return_value.returncode = exec_code
                cli.run_shell(aws, CONFIG)
                return execute.call_args.args[0]

    def test_private_task_enables_exec_and_stops_on_shell_exit(self):
        aws = FakeAWS()
        command = self.shell(aws)
        run = next(call for call in aws.calls if call[:2] == ("ecs", "run-task"))
        self.assertIn("--enable-execute-command", run)
        network = json.loads(run[run.index("--network-configuration") + 1])
        self.assertEqual(network["awsvpcConfiguration"]["assignPublicIp"], "DISABLED")
        self.assertIn("execute-command", command)
        self.assertIn("--interactive", command)
        self.assertEqual(command[-1], "/bin/sh")
        self.assertEqual(aws.calls[-2][:2], ("ecs", "stop-task"))
        self.assertEqual(aws.calls[-1][:2], ("ecs", "deregister-task-definition"))
        self.assertEqual(aws.registered["containerDefinitions"][0]["secrets"][0]["valueFrom"], "secret")

    def test_empty_secret_still_opens_shell(self):
        aws = FakeAWS(has_secret=False)
        self.shell(aws)
        self.assertEqual(aws.registered["containerDefinitions"][0]["secrets"], [])

    def test_exec_failure_and_interrupt_stop_task(self):
        for exception in (RuntimeError, KeyboardInterrupt):
            aws = FakeAWS()
            with self.subTest(exception=exception), self.assertRaises(exception):
                self.shell(aws, exec_code=1, exception=KeyboardInterrupt() if exception is KeyboardInterrupt else None)
            self.assertTrue(any(call[:2] == ("ecs", "stop-task") for call in aws.calls))
            self.assertTrue(any(call[:2] == ("ecs", "deregister-task-definition") for call in aws.calls))

    def test_agent_timeout_stops_task_without_opening_shell(self):
        aws = FakeAWS(ready=False)
        with patch.object(cli.time, "monotonic", side_effect=[0, 301]):
            with self.assertRaisesRegex(RuntimeError, "Timed out"):
                self.shell(aws)
        self.assertTrue(any(call[:2] == ("ecs", "stop-task") for call in aws.calls))

    def test_uncertain_launch_is_not_retried_or_deregistered(self):
        aws = FakeAWS(launch_error=True)
        with self.assertRaisesRegex(RuntimeError, "uncertain"):
            self.shell(aws)
        self.assertEqual(sum(call[:2] == ("ecs", "run-task") for call in aws.calls), 1)
        self.assertFalse(any(call[:2] == ("ecs", "deregister-task-definition") for call in aws.calls))

    def test_rejected_launch_deregisters_revision(self):
        aws = FakeAWS(rejected=True)
        with self.assertRaisesRegex(RuntimeError, "capacity"):
            self.shell(aws)
        self.assertTrue(any(call[:2] == ("ecs", "deregister-task-definition") for call in aws.calls))

    def test_registration_uses_immutable_image_and_discards_response_fields(self):
        definition = {"family": "cli", "revision": 5, "taskDefinitionArn": "old",
                      "containerDefinitions": [{"name": "cli", "image": IMAGE}]}
        request = cli.registration(definition, IMAGE)
        self.assertNotIn("revision", request)
        self.assertNotIn("taskDefinitionArn", request)
        with self.assertRaises(ValueError):
            cli.registration(definition, "repository:latest")


if __name__ == "__main__":
    unittest.main()
