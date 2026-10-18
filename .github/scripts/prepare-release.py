"""Validate a release tag and optionally apply its version to the workspace."""

import argparse
import os
from pathlib import Path
import re


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--validate-only", action="store_true")
args = parser.parse_args()

number = r"(?:0|[1-9][0-9]*)"
identifier = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
semver = (
    rf"{number}\.{number}\.{number}"
    rf"(?:-{identifier}(?:\.{identifier})*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)
tag = os.environ.get("RELEASE_TAG", "")
if not re.fullmatch(rf"v{semver}", tag):
    raise SystemExit(f"Invalid release tag: {tag!r}; expected vMAJOR.MINOR.PATCH")
version = tag[1:]

if not args.validate_only:
    manifest = Path("Cargo.toml")
    lines = manifest.read_text(encoding="utf-8").splitlines(keepends=True)
    in_package = False
    replaced = False
    for index, line in enumerate(lines):
        if line.lstrip().startswith("["):
            in_package = line.strip() == "[workspace.package]"
        elif in_package and re.match(r"\s*version\s*=", line):
            lines[index] = f'version = "{version}"\n'
            replaced = True
            break
    if not replaced:
        raise SystemExit("Missing [workspace.package].version in Cargo.toml")
    manifest.write_text("".join(lines), encoding="utf-8")

if output_path := os.environ.get("GITHUB_OUTPUT"):
    with open(output_path, "a", encoding="utf-8") as output:
        output.write(f"version={version}\n")
        output.write(f"prerelease={str('-' in version.split('+', 1)[0]).lower()}\n")
print(f"Release version: {version}")
