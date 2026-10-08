"""Check release failure paths without credentials or AWS calls."""
import unittest
from unittest.mock import patch

from release import Lambda, release


class FakeLambda:
    def __init__(self, fail_check=None, alias_conflict=False, image_conflict=False, unchanged=False):
        self.calls = []
        self.checks = []
        self.fail_check = fail_check
        self.alias_conflict = alias_conflict
        self.image_conflict = image_conflict
        self.unchanged = unchanged

    def call(self, operation, *args):
        self.calls.append((operation, args))
        if operation == "get-alias":
            return {"FunctionVersion": "7", "RevisionId": "old-alias"}
        if operation == "get-function-configuration":
            return {"RevisionId": "function-revision"}
        if operation == "list-versions-by-function":
            return {"Versions": [{"Version": "8", "CodeSha256": "same-code", "ConfigSha256": "same-config"}] if self.unchanged else []}
        if operation == "get-function":
            return {"Configuration": {"RevisionId": "function-revision", "CodeSha256": "same-code", "ConfigSha256": "same-config"},
                    "Code": {"ResolvedImageUri": "other-image" if self.image_conflict else "repo@sha256:123"}}
        if operation == "publish-version":
            return {"Version": "8"}
        if operation == "update-alias":
            if self.alias_conflict:
                raise RuntimeError("Alias was changed by another deploy")
            return {"RevisionId": "promoted-alias"}
        return {}

    def check(self, qualifier):
        self.checks.append(qualifier)
        if qualifier == self.fail_check:
            raise RuntimeError("Smoke check failed")


class ReleaseTests(unittest.TestCase):
    def test_candidate_failure_never_changes_live(self):
        client = FakeLambda(fail_check="8")
        with self.assertRaises(RuntimeError):
            release(client, "repo@sha256:123")
        self.assertFalse(any(op == "update-alias" for op, _ in client.calls))

    def test_success_checks_candidate_then_promotes_with_revision_guard(self):
        client = FakeLambda()
        self.assertEqual(release(client, "repo@sha256:123"), ("8", "7"))
        self.assertEqual(client.checks, ["8", "live"])
        promotion = next(args for op, args in client.calls if op == "update-alias")
        self.assertIn("old-alias", promotion)
        self.assertIn("function-revision", next(args for op, args in client.calls if op == "publish-version"))

    def test_live_failure_rolls_back_only_our_promotion(self):
        client = FakeLambda(fail_check="live")
        with self.assertRaises(RuntimeError):
            release(client, "repo@sha256:123")
        changes = [args for op, args in client.calls if op == "update-alias"]
        self.assertEqual(len(changes), 2)
        self.assertIn("7", changes[1])
        self.assertIn("promoted-alias", changes[1])

    def test_racing_alias_change_is_never_overwritten(self):
        client = FakeLambda(alias_conflict=True)
        with self.assertRaises(RuntimeError):
            release(client, "repo@sha256:123")
        self.assertEqual(sum(op == "update-alias" for op, _ in client.calls), 1)

    def test_racing_image_change_is_never_published_or_promoted(self):
        client = FakeLambda(image_conflict=True)
        with self.assertRaises(RuntimeError):
            release(client, "repo@sha256:123")
        self.assertFalse(any(op in ("publish-version", "update-alias") for op, _ in client.calls))

    def test_unchanged_release_reuses_checked_version(self):
        client = FakeLambda(unchanged=True)
        self.assertEqual(release(client, "repo@sha256:123"), ("8", "7"))
        self.assertFalse(any(op == "publish-version" for op, _ in client.calls))
        self.assertEqual(client.checks, ["8", "live"])

    @patch("release.subprocess.run")
    def test_waiter_uses_aws_cli_subcommand_order(self, run):
        run.return_value.stdout = ""
        Lambda("ultrafinance").call("wait", "function-updated-v2")
        self.assertEqual(run.call_args.args[0][:4], ["aws", "lambda", "wait", "function-updated-v2"])


if __name__ == "__main__":
    unittest.main()
