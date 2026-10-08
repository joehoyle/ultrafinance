import json
import os
import unittest
from unittest.mock import patch

import tofu


class CredentialBridgeTests(unittest.TestCase):
    @patch.dict(os.environ, {"AWS_PROFILE": "joehoyle", "AWS_DEFAULT_PROFILE": "default"})
    @patch("tofu.os.execvpe")
    @patch("tofu.subprocess.run")
    @patch("tofu.sys.argv", ["tofu.py", "plan", "-input=false"])
    def test_credentials_are_passed_in_memory_and_profile_chain_is_removed(self, run, execute):
        run.return_value.stdout = json.dumps({"AccessKeyId": "test-key", "SecretAccessKey": "test-secret", "SessionToken": "test-session"})
        tofu.main()
        self.assertEqual(run.call_args.args[0][:4], ["aws", "configure", "export-credentials", "--profile"])
        environment = execute.call_args.args[2]
        self.assertEqual(environment["AWS_SESSION_TOKEN"], "test-session")
        self.assertEqual(environment["AWS_SECRET_ACCESS_KEY"], "test-secret")
        self.assertNotIn("AWS_PROFILE", environment)
        self.assertNotIn("AWS_DEFAULT_PROFILE", environment)
        self.assertEqual(execute.call_args.args[1][-2:], ["plan", "-input=false"])


if __name__ == "__main__":
    unittest.main()
