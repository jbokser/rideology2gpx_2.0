#!/usr/bin/env python3
"""Prepare a reviewable Rust release without creating or pushing a Git tag."""

import argparse
import re
import subprocess
import sys
from pathlib import Path
from typing import Optional

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "Cargo.toml"
CHANGELOG = ROOT / "CHANGELOG.md"
VERSION_RE = re.compile(r"^\d+\.\d+\.\d+(?:-(?:alpha|beta|rc)\.\d+)?$")
GROUPS = {
    "feat": "Added",
    "fix": "Fixed",
    "perf": "Improved",
    "docs": "Documentation",
}


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def suggest_version(current: str, previous: Optional[str]) -> str:
    if previous is None:
        return current if "-" in current else f"{current}-beta.1"
    match = re.fullmatch(r"v(\d+)\.(\d+)\.(\d+)(?:-(alpha|beta|rc)\.(\d+))?", previous)
    if match is None:
        raise ValueError(f"Unsupported previous tag: {previous}")
    major, minor, patch, stage, number = match.groups()
    if stage is not None:
        return f"{major}.{minor}.{patch}-{stage}.{int(number) + 1}"
    return f"{major}.{minor}.{int(patch) + 1}"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", nargs="?", help="New version, for example 0.1.0-beta.1")
    args = parser.parse_args()
    manifest = MANIFEST.read_text()
    package_section = manifest.split("[package]", 1)[1].split("\n[", 1)[0]
    current_match = re.search(r'(?m)^version\s*=\s*"([^"]+)"', package_section)
    if current_match is None:
        parser.error("Cargo.toml is missing a package version")
    current = current_match.group(1)
    previous_tags = git("tag", "--list", "v[0-9]*", "--sort=-version:refname").splitlines()
    previous = previous_tags[0] if previous_tags else None
    if args.version is None:
        suggestion = suggest_version(current, previous)
        print(f"Previous tag: {previous or '(none)'}")
        print(f"Suggested next tag: v{suggestion}")
        print(f"To prepare it: python3 scripts/prepare_release.py {suggestion}")
        print("The suggestion is read-only; choose another version explicitly if needed.")
        return 0
    version = args.version
    if not VERSION_RE.fullmatch(version):
        parser.error("version must be MAJOR.MINOR.PATCH, optionally with -alpha.N, -beta.N, or -rc.N")
    if git("status", "--porcelain"):
        parser.error("commit or stash all changes before preparing a release")
    if subprocess.run(["git", "rev-parse", "--verify", f"refs/tags/v{version}"], cwd=ROOT, capture_output=True).returncode == 0:
        parser.error(f"tag v{version} already exists")

    revision_range = f"{previous}..HEAD" if previous else "HEAD"
    subjects = git("log", "--reverse", "--format=%s", revision_range).splitlines()
    sections = {}
    for subject in subjects:
        if re.match(r"^chore: prepare v\d+\.\d+\.\d+", subject):
            continue
        match = re.match(r"^(feat|fix|perf|docs)(?:\([^)]*\))?!?:\s+(.+)$", subject)
        if match:
            sections.setdefault(GROUPS[match.group(1)], []).append(match.group(2))
        elif subject and subject != "Initial commit":
            sections.setdefault("Other changes", []).append(subject)
    if not sections:
        parser.error("no release notes found in commits since the previous tag")

    changelog = CHANGELOG.read_text()
    heading = f"## [{version}]"
    if heading in changelog:
        parser.error(f"CHANGELOG.md already contains {heading}")
    notes = heading + "\n\n"
    for group in ("Added", "Fixed", "Improved", "Documentation", "Other changes"):
        if group in sections:
            notes += f"### {group}\n\n" + "\n".join(
                f"- {item[:1].upper()}{item[1:]}" for item in sections[group]
            ) + "\n\n"
    marker = "## [Unreleased]\n"
    if marker not in changelog:
        parser.error("CHANGELOG.md is missing its Unreleased section")
    updated_manifest, count = re.subn(
        r'(?m)^(version\s*=\s*")[^"]+("\s*)$',
        lambda match: match.group(1) + version + match.group(2),
        manifest,
        count=1,
    )
    if count != 1:
        parser.error("could not update the package version in Cargo.toml")
    MANIFEST.write_text(updated_manifest)
    CHANGELOG.write_text(changelog.replace(marker, marker + "\n" + notes, 1))
    try:
        subprocess.run(["cargo", "check", "--offline"], cwd=ROOT, check=True)
    except (OSError, subprocess.CalledProcessError) as error:
        print(f"cargo check failed: {error}; review the local changes before retrying", file=sys.stderr)
        return 1
    lock = (ROOT / "Cargo.lock").read_text()
    package_entries = lock.split("[[package]]")
    if not any(re.search(r'(?m)^name = "rideology2gpx"$', entry) and re.search(rf'(?m)^version = "{re.escape(version)}"$', entry) for entry in package_entries):
        print("Cargo.lock does not contain the new package version", file=sys.stderr)
        return 1
    print(f"Prepared v{version}. Review the diff, commit Cargo.toml, Cargo.lock and CHANGELOG.md, then push the matching tag.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
