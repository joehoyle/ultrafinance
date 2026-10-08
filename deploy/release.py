#!/usr/bin/env python3
"""Publish, check, and promote an ECR digest without running OpenTofu."""
import argparse
import base64
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile


class Lambda:
    def __init__(self, function):
        self.function = function

    def call(self, operation, *arguments):
        if operation == "wait":
            command = ["aws", "lambda", "wait", *arguments, "--function-name", self.function,
                       "--no-cli-pager", "--output", "json"]
        else:
            command = ["aws", "lambda", operation, "--function-name", self.function,
                       "--no-cli-pager", "--output", "json", *arguments]
        result = subprocess.run(command, check=True, capture_output=True, text=True)
        return json.loads(result.stdout) if result.stdout.strip() else {}

    def check(self, qualifier):
        # HTTP API v2 event format used by function URLs and Lambda Web Adapter.
        for method, path, body, expected in [
            ("GET", "/health", None, 200),
            ("GET", "/", None, 200),
            # Startup/health are intentionally database-lazy. Exercise a real
            # catalog read before promotion to catch URL/schema/connect failures.
            ("GET", "/v1/merchants", None, 200),
            ("POST", "/v1/enrich", '{"description":""}', 422),
        ]:
            event = {
                "version": "2.0", "routeKey": "$default", "rawPath": path,
                "rawQueryString": "", "headers": {"content-type": "application/json", "host": "localhost"},
                "requestContext": {"routeKey": "$default", "stage": "$default", "requestId": "deploy-check",
                    "timeEpoch": 0, "domainName": "localhost", "accountId": "deploy-check",
                    "http": {"method": method, "path": path, "protocol": "HTTP/1.1", "sourceIp": "127.0.0.1", "userAgent": "deploy-check"}},
                "body": body, "isBase64Encoded": False,
            }
            with tempfile.TemporaryDirectory(prefix="ultrafinance-check-") as directory:
                payload = Path(directory) / "event.json"
                response = Path(directory) / "response.json"
                payload.write_text(json.dumps(event))
                metadata = self.call("invoke", "--qualifier", qualifier,
                                     "--payload", f"fileb://{payload}", str(response))
                if metadata.get("FunctionError"):
                    raise RuntimeError(f"Lambda {qualifier} failed its {path} check")
                result = json.loads(response.read_text())
                if result.get("statusCode") != expected:
                    raise RuntimeError(f"Lambda {qualifier} returned {result.get('statusCode')} for {path}, expected {expected}")
                if path == "/health":
                    content = result.get("body", "")
                    if result.get("isBase64Encoded"):
                        content = base64.b64decode(content)
                    if json.loads(content).get("status") != "ok":
                        raise RuntimeError(f"Lambda {qualifier} health check failed")


def release(client, image):
    previous = client.call("get-alias", "--name", "live")
    current = client.call("get-function-configuration")
    # Revision guards prevent racing a local deploy, CI, or an infrastructure update.
    client.call("update-function-code", "--image-uri", image, "--revision-id", current["RevisionId"])
    client.call("wait", "function-updated-v2")
    updated = client.call("get-function")
    if updated.get("Code", {}).get("ResolvedImageUri") != image:
        raise RuntimeError("Function image changed during deployment; live was not promoted")
    # Repeated deployments of an unchanged image/config can reuse a published
    # version: Lambda refuses to publish an identical snapshot again.
    config = updated["Configuration"]
    versions = client.call("list-versions-by-function").get("Versions", [])
    fields = (
        "CodeSha256", "Role", "Runtime", "Handler", "Description", "Timeout",
        "MemorySize", "Environment", "VpcConfig", "DeadLetterConfig", "KMSKeyArn",
        "TracingConfig", "Layers", "FileSystemConfigs", "ImageConfigResponse",
        "Architectures", "EphemeralStorage", "SnapStart", "LoggingConfig",
    )
    # ConfigSha256 is supplied by newer Lambda APIs and covers additional config.
    if config.get("ConfigSha256"):
        fields = ("CodeSha256", "ConfigSha256")
    matches = [item for item in versions if item.get("Version", "").isdigit()
               and all(item.get(field) == config.get(field) for field in fields)]
    if matches:
        version = max(matches, key=lambda item: int(item["Version"]))["Version"]
    else:
        published = client.call("publish-version", "--revision-id", config["RevisionId"],
                                "--description", config.get("Description", ""))
        version = published["Version"]
    client.check(version)
    promoted = client.call("update-alias", "--name", "live", "--function-version", version,
                           "--revision-id", previous["RevisionId"],
                           "--routing-config", '{"AdditionalVersionWeights":{}}')
    try:
        client.check("live")
    except Exception:
        # Only roll back if no other release changed the alias after our promotion.
        client.call("update-alias", "--name", "live", "--function-version", previous["FunctionVersion"],
                    "--revision-id", promoted["RevisionId"],
                    "--routing-config", json.dumps(previous.get("RoutingConfig", {"AdditionalVersionWeights": {}})))
        raise
    return version, previous["FunctionVersion"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", help="Immutable ECR digest URI")
    parser.add_argument("--function", default=os.environ.get("LAMBDA_FUNCTION_NAME", "ultrafinance"))
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9]{12}\.dkr\.ecr\.[a-z0-9-]+\.amazonaws\.com/[a-z0-9/_-]+@sha256:[a-f0-9]{64}", args.image):
        parser.error("image must be an ECR sha256 digest URI")
    version, previous = release(Lambda(args.function), args.image)
    print(f"Deployed version {version}; previous version {previous}")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
            summary.write(f"Deployed Lambda version **{version}** to `live`. Previous version: **{previous}**.\n")


if __name__ == "__main__":
    try:
        main()
    except (subprocess.CalledProcessError, RuntimeError, ValueError, KeyError) as error:
        # Do not print invoke bodies or Lambda environment configuration.
        sys.exit(f"Release failed: {error}")
