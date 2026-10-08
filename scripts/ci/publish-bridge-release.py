#!/usr/bin/env python3
"""Resume a same-tag draft, verify downloaded assets, then publish it."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
from urllib.parse import quote


class PublishError(Exception):
    pass


def command(argv, cwd=None):
    try:
        return subprocess.run(argv, cwd=cwd, capture_output=True, text=True)
    except OSError as error:
        raise PublishError("Required release command could not run.") from error


def checked(argv, operation, cwd=None):
    result = command(argv, cwd)
    if result.returncode != 0:
        # Never echo gh stderr, response bodies or credential-bearing arguments.
        raise PublishError(f"Release operation failed: {operation}.")
    return result.stdout.strip()


class Publisher:
    def __init__(self, repo, tag, source_sha, dist, source_dir):
        self.repo, self.tag, self.source_sha = repo, tag, source_sha
        self.dist, self.source_dir = Path(dist).resolve(), Path(source_dir).resolve()
        self.release_id = None

    def validate_source(self):
        if not re.fullmatch(r"[\w.-]+/[\w.-]+", self.repo, flags=re.ASCII):
            raise PublishError("Invalid release repository.")
        if not self.tag.startswith("v") or not re.fullmatch(r"[0-9a-f]{40}", self.source_sha):
            raise PublishError("Invalid release tag or source revision.")
        for revision in ("HEAD", f"refs/tags/{self.tag}^{{commit}}"):
            actual = checked(["git", "rev-parse", "--verify", revision], "source binding", self.source_dir)
            if actual != self.source_sha:
                raise PublishError("Release source does not match the checked tag revision.")

    def state(self):
        endpoint = f"repos/{self.repo}/releases/tags/{quote(self.tag, safe='')}"
        result = command(["gh", "api", "--include", endpoint])
        header, separator, body = result.stdout.replace("\r\n", "\n").partition("\n\n")
        match = re.match(r"HTTP/[\d.]+ (\d{3})\b", header)
        if not separator or not match:
            raise PublishError("Release lookup did not provide a valid HTTP response.")
        status = int(match.group(1))
        if status == 404 and result.returncode != 0:
            return None
        if status != 200 or result.returncode != 0:
            raise PublishError(f"Release lookup failed (HTTP {status}).")
        try:
            state = json.loads(body)
        except ValueError as error:
            raise PublishError("Release lookup returned invalid JSON.") from error
        if (not isinstance(state, dict) or state.get("tag_name") != self.tag
                or type(state.get("draft")) is not bool or type(state.get("id")) is not int
                or state["id"] <= 0):
            raise PublishError("Release lookup returned invalid draft identity.")
        return state

    def require_draft(self):
        state = self.state()
        if state is None or not state["draft"]:
            raise PublishError("Release is missing or already published; refusing mutation.")
        if self.release_id is not None and state["id"] != self.release_id:
            raise PublishError("Release identity changed; refusing mutation.")
        self.release_id = state["id"]

    def local_manifest(self):
        try:
            manifest = (self.dist / "SHA256SUMS").read_bytes()
            match = re.fullmatch(rb"([0-9a-f]{64})  rdpilot-bridge\.exe\n", manifest)
            digest = hashlib.sha256((self.dist / "rdpilot-bridge.exe").read_bytes()).hexdigest()
        except OSError as error:
            raise PublishError("Expected local release assets are unavailable.") from error
        if match is None or digest != match.group(1).decode("ascii"):
            raise PublishError("Local bridge checksum manifest does not match the executable.")
        return manifest, digest

    def publish(self):
        self.validate_source()
        manifest, digest = self.local_manifest()
        state = self.state()
        if state is None:
            checked(["gh", "release", "create", self.tag, "--repo", self.repo, "--draft", "--verify-tag",
                     "--title", f"rdpilot {self.tag}", "--notes",
                     "rdpilot-bridge.exe for x86_64 Windows guests and its SHA256SUMS."], "draft creation")
        elif not state["draft"]:
            raise PublishError("Release is already published; refusing mutation.")
        else:
            self.release_id = state["id"]

        for name in ("rdpilot-bridge.exe", "SHA256SUMS"):
            # --clobber deletes before uploading. Check the draft again before
            # EACH destructive upload, not only before the final publication.
            self.require_draft()
            checked(["gh", "release", "upload", self.tag, str(self.dist / name),
                     "--repo", self.repo, "--clobber"], "draft asset upload")

        with tempfile.TemporaryDirectory(prefix="rdpilot-release-check-") as directory:
            checked(["gh", "release", "download", self.tag, "--repo", self.repo, "--dir", directory,
                     "--pattern", "rdpilot-bridge.exe", "--pattern", "SHA256SUMS"], "asset download")
            try:
                downloaded = Path(directory)
                same_manifest = (downloaded / "SHA256SUMS").read_bytes() == manifest
                same_digest = hashlib.sha256((downloaded / "rdpilot-bridge.exe").read_bytes()).hexdigest() == digest
            except OSError as error:
                raise PublishError("Downloaded release assets are incomplete.") from error
            if not same_manifest or not same_digest:
                raise PublishError("Downloaded release assets failed checksum verification.")

        self.require_draft()
        checked(["gh", "release", "edit", self.tag, "--repo", self.repo,
                 "--draft=false", "--prerelease=false", "--latest"], "publication")
        print("Verified bridge assets published.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--dist", default="dist")
    args = parser.parse_args()
    try:
        Publisher(args.repo, args.tag, args.source_sha, args.dist, Path.cwd()).publish()
    except PublishError as error:
        print(str(error))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
