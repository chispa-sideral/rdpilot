"""Test checked-in aggregate execution and failure-artifact selection."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("offline_proof", Path(__file__).with_name("run-offline-proof.py"))
offline = importlib.util.module_from_spec(spec)
spec.loader.exec_module(offline)


class AggregateTests(unittest.TestCase):
    def setUp(self):
        self.workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        line = next(line.strip().removeprefix("run: ") for line in self.workflow.splitlines()
                    if "run: python3 scripts/ci/check-ci-results.py " in line)
        self.command = [sys.executable, *shlex.split(line)[1:]]
        self.names = self.command[2:]

    def aggregate(self, results):
        return subprocess.run(self.command, cwd=ROOT, env={**os.environ, "NEEDS_JSON": json.dumps(results)},
                              capture_output=True, text=True).returncode

    def test_actual_aggregate_requires_all_successful_jobs(self):
        successful = {name: {"result": "success"} for name in self.names}
        self.assertEqual(self.aggregate(successful), 0)
        self.assertNotEqual(self.aggregate({}), 0)
        for name in self.names:
            for result in ("failure", "skipped", "cancelled", "pending", None):
                with self.subTest(name=name, result=result):
                    altered = {**successful, name: {"result": result}}
                    self.assertNotEqual(self.aggregate(altered), 0)
            missing = {key: value for key, value in successful.items() if key != name}
            self.assertNotEqual(self.aggregate(missing), 0)
        self.assertNotEqual(self.aggregate({**successful, "extra": {"result": "success"}}), 0)
        self.assertNotEqual(self.aggregate([]), 0)

    def test_aggregate_names_cover_every_other_job(self):
        jobs = self.workflow.split("\njobs:\n", 1)[1]
        names = set(re.findall(r"^  ([\w-]+):$", jobs, re.MULTILINE)) - {"success"}
        self.assertEqual(set(self.names), names)
        needs = re.search(r"^    needs: \[([^\]]+)\]$", jobs, re.MULTILINE)
        self.assertEqual(set(part.strip() for part in needs.group(1).split(",")), names)

    def test_release_calls_read_only_checks_and_requires_success(self):
        text = (ROOT / ".github/workflows/release.yml").read_text()
        checks = text.split("  checks:\n", 1)[1].split("\n  bridge:\n", 1)[0]
        self.assertIn("uses: ./.github/workflows/ci.yml", checks)
        self.assertIn("contents: read", checks)
        self.assertNotIn("secrets:", checks)
        self.assertNotIn("GH_TOKEN", checks)
        self.assertIn("needs: [checks]", text)
        self.assertIn("if: ${{ needs.checks.result == 'success' }}", text)
        self.assertIn("workflow_call:", self.workflow)
        self.assertEqual(self.workflow.count("persist-credentials: false"), len(self.names) + 1)
        self.assertEqual(self.workflow.count("ref: ${{ github.sha }}"), len(self.names) + 1)


class EvidenceTests(unittest.TestCase):
    def exercise(self, proof, raw, returncode):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "proof"

            def child(argv, **kwargs):
                self.assertEqual(kwargs["stdout"], subprocess.DEVNULL)
                self.assertEqual(kwargs["stderr"], subprocess.DEVNULL)
                self.assertIn("--fake", argv)
                output.mkdir()
                if raw is not None:
                    (output / "summary.json").write_text(json.dumps(raw))
                return subprocess.CompletedProcess(argv, returncode)

            console = io.StringIO()
            with patch.object(offline.subprocess, "run", side_effect=child), contextlib.redirect_stdout(console):
                code = offline.run_proof(proof, ROOT / "target/debug", output)
            selected = (output / "artifact-summary.json").read_text()
            return code, selected, console.getvalue()

    def test_failure_and_scan_failure_artifacts_exclude_raw_payloads(self):
        secret = "unredacted-token-password-lease-typed-marker"
        for proof in offline.CHECKS:
            for returncode in (0, 1):
                with self.subTest(proof=proof, code=returncode):
                    raw = {"mode": "fake", "status": "passed", "failure": secret,
                           "checks": [{"check": offline.CHECKS[proof][0], "passed": True, "tool_result": secret},
                                      {"check": secret, "passed": True}], "lease": secret}
                    code, artifact, console = self.exercise(proof, raw, returncode)
                    self.assertNotEqual(code, 0)  # No completed evidence scan.
                    self.assertEqual(json.loads(artifact)["status"], "failed")
                    self.assertNotIn(secret, artifact + console)
                    self.assertNotIn("tool_result", artifact)

    def test_success_requires_fake_pass_and_every_check(self):
        for proof in offline.CHECKS:
            raw = {"mode": "fake", "status": "passed",
                   "checks": [{"check": name, "passed": True} for name in offline.CHECKS[proof]]}
            code, artifact, _ = self.exercise(proof, raw, 0)
            self.assertEqual(code, 0)
            self.assertEqual(json.loads(artifact)["status"], "passed")
            scan_only = {**raw, "checks": [{"check": offline.CHECKS[proof][-1], "passed": True}]}
            code, artifact, _ = self.exercise(proof, scan_only, 0)
            self.assertNotEqual(code, 0)
            self.assertEqual(json.loads(artifact)["status"], "failed")
            code, artifact, _ = self.exercise(proof, {**raw, "mode": "live"}, 0)
            self.assertNotEqual(code, 0)
            self.assertEqual(json.loads(artifact)["mode"], "fake")
            code, _, _ = self.exercise(proof, raw, 1)
            self.assertNotEqual(code, 0)

    def test_missing_or_invalid_summary_produces_safe_failure(self):
        for raw in (None, [], "raw error or token", {"checks": "raw error or token"}):
            with self.subTest(raw=raw):
                code, artifact, _ = self.exercise("viewer", raw, 1)
                self.assertNotEqual(code, 0)
                self.assertNotIn("raw error or token", artifact)
                self.assertEqual(json.loads(artifact)["completed_checks"], [])


if __name__ == "__main__":
    unittest.main()
