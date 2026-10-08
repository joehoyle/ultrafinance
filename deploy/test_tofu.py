import json
import os
import unittest
from pathlib import Path
import tempfile
from unittest.mock import patch

import tofu


class CredentialBridgeTests(unittest.TestCase):
    @patch.dict(os.environ, {"AWS_PROFILE": "joehoyle", "AWS_DEFAULT_PROFILE": "default"})
    @patch("tofu.os.execvpe")
    @patch("tofu.subprocess.run")
    @patch("tofu.sys.argv", ["tofu.py", "plan", "-input=false"])
    def test_credentials_are_passed_in_memory_and_profile_chain_is_removed(self, run, execute):
        run.return_value.stdout = json.dumps({"AccessKeyId": "test-key", "SecretAccessKey": "test-secret", "SessionToken": "test-session"})
        with patch("tofu.load_provider_key"):
            tofu.main()
        self.assertEqual(run.call_args.args[0][:4], ["aws", "configure", "export-credentials", "--profile"])
        environment = execute.call_args.args[2]
        self.assertEqual(environment["AWS_SESSION_TOKEN"], "test-session")
        self.assertEqual(environment["AWS_SECRET_ACCESS_KEY"], "test-secret")
        self.assertNotIn("AWS_PROFILE", environment)
        self.assertNotIn("AWS_DEFAULT_PROFILE", environment)
        self.assertEqual(execute.call_args.args[1][-2:], ["plan", "-input=false"])

    def test_dotenv_credential_is_forwarded_to_sensitive_tofu_variable(self):
        with tempfile.TemporaryDirectory() as directory:
            dotenv = Path(directory) / ".env"
            dotenv.write_text('UNRELATED=value\nexport TYPESAFE_API_KEY="test-provider-key" # comment\n')
            environment = {}
            tofu.load_provider_key(environment, dotenv)
            self.assertEqual(environment, {"TF_VAR_typesafe_api_key": "test-provider-key"})

    def test_explicit_environment_overrides_dotenv(self):
        with tempfile.TemporaryDirectory() as directory:
            dotenv = Path(directory) / ".env"
            dotenv.write_text('TYPESAFE_API_KEY=file-key\n')
            environment = {"TYPESAFE_API_KEY": "environment-key"}
            tofu.load_provider_key(environment, dotenv)
            self.assertEqual(environment["TF_VAR_typesafe_api_key"], "environment-key")
            environment = {"TF_VAR_typesafe_api_key": "", "TYPESAFE_API_KEY": "environment-key"}
            tofu.load_provider_key(environment, dotenv)
            self.assertEqual(environment["TF_VAR_typesafe_api_key"], "")


if __name__ == "__main__":
    unittest.main()
