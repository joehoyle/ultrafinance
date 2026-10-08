#!/usr/bin/env python3
"""Run OpenTofu using short-lived credentials from an AWS CLI login profile."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", default=os.environ.get("AWS_PROFILE", "joehoyle"))
    parser.add_argument("command", nargs=argparse.REMAINDER, help="OpenTofu subcommand and arguments")
    args = parser.parse_args()
    if not args.command:
        parser.error("provide an OpenTofu subcommand, such as plan or apply")
    # Credentials stay in process memory, never files or command output.
    exported = subprocess.run(
        ["aws", "configure", "export-credentials", "--profile", args.profile, "--format", "process"],
        check=True, capture_output=True, text=True,
    )
    credentials = json.loads(exported.stdout)
    environment = os.environ.copy()
    for key in ["AWS_PROFILE", "AWS_DEFAULT_PROFILE"]:
        environment.pop(key, None)
    environment.update({
        "AWS_ACCESS_KEY_ID": credentials["AccessKeyId"],
        "AWS_SECRET_ACCESS_KEY": credentials["SecretAccessKey"],
        "AWS_SESSION_TOKEN": credentials.get("SessionToken", ""),
        "AWS_EC2_METADATA_DISABLED": "true",
    })
    infrastructure = Path(__file__).resolve().parent.parent / "infra"
    os.execvpe("tofu", ["tofu", f"-chdir={infrastructure}", *args.command], environment)


if __name__ == "__main__":
    try:
        main()
    except (subprocess.CalledProcessError, ValueError, KeyError):
        sys.exit("Could not export AWS profile credentials. Check your AWS CLI login.")
