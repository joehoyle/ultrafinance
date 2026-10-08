import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("prod_cli", Path(__file__).with_name("prod-cli.py"))
cli = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cli)

IMAGE = "123456789012.dkr.ecr.ca-central-1.amazonaws.com/ultrafinance@sha256:" + "a" * 64
CONFIG = {
    "cluster": "cluster", "task_definition": "definition", "database_secret": "secret",
    "input_bucket": "bucket", "subnets": ["private"], "security_groups": ["db-client"],
    "log_group": "logs",
    "function_name": "function",
}


class FakeAWS:
    def __init__(self, exits=(0, 0), launch_error=False, running=False):
        self.calls = []
        self.exits = exits
        self.launch_error = launch_error
        self.running = running

    def call(self, *args):
        self.calls.append(args)
        operation = args[:2]
        if operation == ("secretsmanager", "describe-secret"):
            return {"VersionIdsToStages": {"v1": ["AWSCURRENT"]}}
        if operation == ("ecs", "describe-task-definition"):
            return {"taskDefinition": {"containerDefinitions": [{"name": "cli", "image": IMAGE}]}}
        if operation == ("lambda", "get-function"):
            return IMAGE
        if operation == ("ecs", "run-task"):
            if self.launch_error:
                raise RuntimeError("uncertain launch")
            return {"tasks": [{"taskArn": "task/123"}]}
        if operation == ("ecs", "describe-tasks"):
            if self.running:
                return {"tasks": [{"lastStatus": "RUNNING"}]}
            return {"tasks": [{
                "lastStatus": "STOPPED", "stopCode": "EssentialContainerExited",
                "containers": [{"name": "input", "exitCode": self.exits[0]},
                               {"name": "cli", "exitCode": self.exits[1]}],
            }]}
        if operation == ("logs", "filter-log-events"):
            return {"events": [{"message": "result"}]}
        return {}


class ProductionCLITests(unittest.TestCase):
    def test_commands_require_input_and_keep_credentials_out_of_overrides(self):
        self.assertEqual(cli.validate_command(["datasets", "apply", "@input"], True),
                         ["datasets", "apply", "/input/source"])
        for command, has_input in [
            (["datasets", "apply", "@input"], False),
            (["merchants", "list"], True),
            (["merchants", "list", "--database-url=postgresql://secret"], False),
            (["datasets", "import"], False),
        ]:
            with self.assertRaises(ValueError):
                cli.validate_command(command, has_input)

    def test_sqlite_snapshot_includes_wal_without_mutating_source(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.sqlite"
            destination = Path(directory) / "snapshot.sqlite"
            connection = sqlite3.connect(source)
            try:
                connection.execute("PRAGMA journal_mode=WAL")
                connection.execute("CREATE TABLE records (value TEXT)")
                connection.execute("INSERT INTO records VALUES ('committed')")
                connection.commit()
                connection.execute("INSERT INTO records VALUES ('uncommitted')")
                cli.snapshot_sqlite(source, destination)
                with sqlite3.connect(destination) as snapshot:
                    self.assertEqual(snapshot.execute("SELECT value FROM records").fetchall(), [("committed",)])
                self.assertEqual(connection.execute("SELECT count(*) FROM records").fetchone()[0], 2)
            finally:
                connection.close()

    def job(self, aws, source=None, timeout=3600):
        with patch.object(cli.time, "sleep"), contextlib.redirect_stdout(io.StringIO()):
            cli.run_job(aws, CONFIG, ["datasets", "apply", "/input/source"] if source else ["merchants", "list"], source, None, timeout)

    def test_success_uses_private_network_and_cleans_staged_file(self):
        aws = FakeAWS()
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "knowledge.json"
            source.write_text("[]")
            self.job(aws, source)
        run = next(call for call in aws.calls if call[:2] == ("ecs", "run-task"))
        network = json.loads(run[run.index("--network-configuration") + 1])
        self.assertEqual(network["awsvpcConfiguration"]["assignPublicIp"], "DISABLED")
        self.assertEqual(sum(call[:2] == ("ecs", "run-task") for call in aws.calls), 1)
        self.assertTrue(any(call[:2] == ("s3", "rm") for call in aws.calls))

    def test_nonzero_cli_and_failed_download_are_failures(self):
        for exits in [(0, 1), (1, None)]:
            with self.subTest(exits=exits), self.assertRaisesRegex(RuntimeError, "Task failed"):
                self.job(FakeAWS(exits=exits))

    def test_uncertain_launch_keeps_input_and_never_retries(self):
        aws = FakeAWS(launch_error=True)
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "knowledge.json"
            source.write_text("[]")
            with self.assertRaisesRegex(RuntimeError, "uncertain launch"):
                self.job(aws, source)
        self.assertFalse(any(call[:2] == ("s3", "rm") for call in aws.calls))
        self.assertEqual(sum(call[:2] == ("ecs", "run-task") for call in aws.calls), 1)

    def test_timeout_does_not_stop_or_retry_task(self):
        aws = FakeAWS(running=True)
        with patch.object(cli.time, "monotonic", side_effect=[0, 2]):
            with self.assertRaisesRegex(RuntimeError, "continues running"):
                self.job(aws, timeout=1)
        self.assertFalse(any(call[:2] == ("ecs", "stop-task") for call in aws.calls))
        self.assertEqual(sum(call[:2] == ("ecs", "run-task") for call in aws.calls), 1)

    def test_image_registration_preserves_secrets_and_rejects_tags(self):
        definition = {"family": "cli", "revision": 5, "taskDefinitionArn": "old",
                      "containerDefinitions": [{"name": "cli", "image": IMAGE,
                                                "secrets": [{"name": "ULTRAFINANCE_DATABASE_URL", "valueFrom": "secret"}]}]}
        request = cli.registration(definition, IMAGE)
        self.assertNotIn("revision", request)
        self.assertNotIn("taskDefinitionArn", request)
        self.assertEqual(request["containerDefinitions"][0]["secrets"][0]["valueFrom"], "secret")
        with self.assertRaises(ValueError):
            cli.registration(definition, "repository:latest")


if __name__ == "__main__":
    unittest.main()
