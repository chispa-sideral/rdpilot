"""Test the evidence-matrix mapping check and the rendering of a run's results.

The tests build their own temporary trees and take every run fact as an argument: none
reads the Actions environment or calls the network, although the unittest steps of the
CI jobs run with GITHUB_REPOSITORY and GITHUB_RUN_ID set.
"""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import re
import shutil
import tempfile
import tomllib
import unittest
import urllib.error


ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("evidence_matrix", Path(__file__).with_name("evidence-matrix.py"))
em = importlib.util.module_from_spec(spec)
spec.loader.exec_module(em)

CI = """name: CI

jobs:
  linux:
    name: Linux checks
    steps:
      - uses: actions/checkout@v4
      - name: Demo tests
        run: cargo test
  offline:
    name: Offline proofs
    steps:
      - name: Offline viewer proof
        run: python proof
  desktop:
    name: Live proofs
    steps:
      - name: Live proof
        run: python live
  evidence-matrix:
    name: Evidence matrix (report only)
    needs: [linux, offline, desktop]
    steps:
      - name: Render
        run: python render
  success:
    name: Required checks success
    needs: [linux, offline, desktop]
    steps:
      - name: Require
        run: python check
"""

RULES = """schema = 1
split_packages = []
placement = ["Offline first."]

[tiers]
offline = "No RDP target."
hosted-live = "The desktop job."

[modes]
synthetic = "No RDP server."
live = "A real session."

[cadences]
every-push = "Every run."
on-demand = "By hand."

[budget]
offline_jobs = ["linux", "offline"]
offline_minutes = 20
desktop_jobs = ["desktop"]
desktop_minutes = 90

[[rule]]
kind = "offline-check"
match = "viewer/*"
mode = "synthetic"
tier = "offline"
cadence = "every-push"
runs = [{ job = "offline", step = "Offline viewer proof", proof = "viewer" }]

[[rule]]
kind = "e2e"
match = "run-x.py/*"
mode = "live"
tier = "hosted-live"
cadence = "every-push"
why = "needs a real server"
runs = [{ job = "desktop", step = "Live proof", proof = "cua" }]

[[rule]]
kind = "cargo"
match = "demo/lib"
mode = "synthetic"
tier = "offline"
cadence = "every-push"
runs = [{ job = "linux", step = "Demo tests" }]

[[rule]]
kind = "cargo"
match = "demo/test/soak"
mode = "synthetic"
tier = "offline"
cadence = "every-push"
runs = [{ job = "linux", step = "Demo tests" }]

[[rule]]
kind = "ignored"
match = "crates/demo/tests/soak.rs::*"
mode = "synthetic"
tier = "offline"
cadence = "on-demand"
why = "runs only with --ignored"
runs = []

[[rule]]
kind = "file"
match = "scripts/ci/test_*.py"
mode = "synthetic"
tier = "offline"
cadence = "every-push"
runs = [{ job = "linux", step = "Demo tests" }]
"""

OFFLINE_PROOF = """CHECKS = {"viewer": ("a.connected", "b.connected", "c.hidden")}
"""

E2E = """class Proof:
    def run(self, t, extra):
        self.check('viewer_started')
        self.check(f'{t}.connected')
        self.check('go' + ('_read_only' if extra else ''))
"""


def make_tree(test, rules=RULES):
    directory = tempfile.TemporaryDirectory()
    test.addCleanup(directory.cleanup)
    root = Path(directory.name)
    for relative in ("scripts/e2e", "scripts/ci", "crates/demo/src", "crates/demo/tests", ".github/workflows"):
        (root / relative).mkdir(parents=True)
    (root / ".github/workflows/ci.yml").write_text(CI, encoding="utf-8")
    (root / "scripts/ci/evidence-rules.toml").write_text(rules, encoding="utf-8")
    (root / "scripts/ci/run-offline-proof.py").write_text(OFFLINE_PROOF, encoding="utf-8")
    (root / "scripts/ci/test_x.py").write_text("", encoding="utf-8")
    (root / "scripts/e2e/run-x.py").write_text(E2E, encoding="utf-8")
    (root / "crates/demo/Cargo.toml").write_text('[package]\nname = "demo"\nversion = "0.0.0"\n', encoding="utf-8")
    (root / "crates/demo/src/lib.rs").write_text("#[test]\nfn quick() {}\n", encoding="utf-8")
    (root / "crates/demo/tests/soak.rs").write_text('#[test]\n#[ignore = "slow [x]"]\nfn slow() {}\n', encoding="utf-8")
    return root


class MappingTests(unittest.TestCase):
    def test_real_tree_is_covered(self):
        self.assertEqual(em.check(ROOT), [])

    def test_every_selector_gets_exactly_one_rule(self):
        rules = em.load_rules(ROOT)
        selectors = em.enumerate_selectors(ROOT, set(rules["split_packages"]))
        self.assertTrue(selectors)
        for (kind, selector) in selectors:
            index = em.find_rule(rules, kind, selector)
            self.assertIsNotNone(index, (kind, selector))
            self.assertEqual(rules["rule"][index]["kind"], kind)

    def test_selector_ids_use_the_documented_forms(self):
        rules = em.load_rules(ROOT)
        patterns = {
            "offline-check": r"[a-z]+/[a-z0-9_.]+",
            "e2e": r"run-[a-z0-9-]+\.py/[A-Za-z0-9_.{}]+",
            "cargo": r"[a-z0-9-]+/(?:lib|lib/[a-z0-9_]+|bin/[a-z0-9-]+|test/[a-z0-9_]+)",
            "ignored": r"crates/[^:]+\.rs::\w+",
            "file": r"scripts/(?:ci/test_[a-z_]+\.py|e2e/[a-z-]+\.sh)",
        }
        for (kind, selector) in em.enumerate_selectors(ROOT, set(rules["split_packages"])):
            self.assertRegex(selector, "^(?:" + patterns[kind] + ")$")

    def test_ci_steps_named_by_rules_exist(self):
        rules, jobs = em.load_rules(ROOT), em.workflow_jobs(ROOT)
        for rule in rules["rule"]:
            for run in rule["runs"]:
                self.assertIn(run["step"], jobs[run["job"]]["steps"])

    def test_perf_selectors_follow_their_placement(self):
        rules = em.load_rules(ROOT)
        selectors = em.enumerate_selectors(ROOT, set(rules["split_packages"]))
        expected = {
            "offline-check": ("perf/", "synthetic", "offline", "every-push",
                              [{"job": "offline", "step": "Offline perf workload smoke", "proof": "perf"}]),
            "e2e": ("run-perf-proof.py/", "live", "hosted-live", "on-demand",
                    [{"job": "desktop", "step": "Performance workload (on demand)", "proof": "perf"}]),
        }
        for kind, (prefix, mode, tier, cadence, runs) in expected.items():
            found = [selector for (k, selector) in selectors if k == kind and selector.startswith(prefix)]
            self.assertTrue(found, kind)
            for selector in found:
                rule = rules["rule"][em.find_rule(rules, kind, selector)]
                self.assertEqual((rule["mode"], rule["tier"], rule["cadence"], rule["runs"]),
                                 (mode, tier, cadence, runs), selector)

    def test_perf_results_reach_their_rows(self):
        rules = em.load_rules(ROOT)
        base = Path(tempfile.mkdtemp())
        self.addCleanup(lambda: shutil.rmtree(base, ignore_errors=True))
        offline = base / "offline-proof-summaries/offline-perf"
        offline.mkdir(parents=True)
        (offline / "artifact-summary.json").write_text(json.dumps(
            {"proof": "perf", "status": "passed", "completed_checks": ["w1.comparison_rendered"]}), encoding="utf-8")
        live = base / "desktop-proof-evidence/perf/proof"
        live.mkdir(parents=True)
        (live / "summary.json").write_text(json.dumps(
            {"status": "passed", "checks": [{"check": "w1.clock_offset_bounded", "passed": True}]}), encoding="utf-8")
        proofs = em.proof_results(base)

        def state(job, step):
            return "ok", True
        for kind, selector in (("offline-check", "perf/w1.comparison_rendered"),
                               ("e2e", "run-perf-proof.py/w1.clock_offset_bounded")):
            rule = rules["rule"][em.find_rule(rules, kind, selector)]
            result = em.proof_row_result({"kind": kind, "selector": selector}, rule, proofs, state)
            self.assertEqual(result, ("passed", "", [selector.split("/", 1)[1]]))

    def test_recording_results_reach_their_rows(self):
        base = Path(tempfile.mkdtemp())
        self.addCleanup(lambda: shutil.rmtree(base, ignore_errors=True))
        live = base / "desktop-recording-proof-evidence/recording/proof"
        live.mkdir(parents=True)
        (live / "summary.json").write_text(json.dumps(
            {"status": "passed", "checks": [{"check": "idle_interval_adds_no_video", "passed": True}]}), encoding="utf-8")
        proofs = em.proof_results(base)
        self.assertIn("recording", proofs["e2e"])
        selector = "run-recording-proof.py/idle_interval_adds_no_video"
        rule = {"runs": [{"job": "desktop-recording", "step": "Live recording proof", "proof": "recording"}]}
        result = em.proof_row_result({"kind": "e2e", "selector": selector}, rule, proofs, lambda job, step: ("ok", True))
        self.assertEqual(result, ("passed", "", ["idle_interval_adds_no_video"]))

    def test_cli_check_succeeds(self):
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(em.main(["check"]), 0)


class InjectionTests(unittest.TestCase):
    """A tree that the rules cover passes; each injected item must then be a problem."""

    def test_covered_tree_has_no_problem(self):
        self.assertEqual(em.check(make_tree(self)), [])

    def test_removed_rule_is_a_problem(self):
        root = make_tree(self)
        rules = (root / "scripts/ci/evidence-rules.toml")
        text = rules.read_text(encoding="utf-8")
        start = text.index('[[rule]]\nkind = "e2e"')
        end = text.index("[[rule]]", start + 1)
        rules.write_text(text[:start] + text[end:], encoding="utf-8")
        problems = em.check(root)
        self.assertEqual(len(problems), 4)
        self.assertTrue(all(p.startswith("e2e selector run-x.py/") and "no rule matches" in p for p in problems))

    def test_new_label_under_a_covered_pattern_needs_no_rule(self):
        root = make_tree(self)
        with open(root / "scripts/e2e/run-x.py", "a", encoding="utf-8") as handle:
            handle.write("        self.check('another')\n")
        self.assertEqual(em.check(root), [])

    def test_new_literal_check_label_in_an_unmapped_script_is_a_problem(self):
        root = make_tree(self)
        (root / "scripts/e2e/run-y.py").write_text("def go(run):\n    run.check('fresh')\n", encoding="utf-8")
        self.assertEqual(em.check(root), ["e2e selector run-y.py/fresh: no rule matches (run-y.py:2)"])

    def test_new_generated_check_label_is_a_problem(self):
        root = make_tree(self)
        (root / "scripts/e2e/run-y.py").write_text("def go(run, t):\n    run.check(f'{t}.extra_{t}')\n", encoding="utf-8")
        problems = em.check(root)
        self.assertEqual(len(problems), 1)
        self.assertIn("run-y.py/{}.extra_{}", problems[0])
        self.assertIn("run-y.py:2", problems[0])

    def test_new_cargo_target_is_a_problem(self):
        root = make_tree(self)
        (root / "crates/demo/tests/more.rs").write_text("#[test]\nfn more() {}\n", encoding="utf-8")
        self.assertEqual(em.check(root), ["cargo selector demo/test/more: no rule matches"])

    def test_new_ignored_test_is_a_problem(self):
        root = make_tree(self)
        with open(root / "crates/demo/src/lib.rs", "a", encoding="utf-8") as handle:
            handle.write("#[test]\n#[ignore]\nasync fn other() {}\n")
        self.assertEqual(em.check(root), ["ignored selector crates/demo/src/lib.rs::other: no rule matches"])

    def test_new_ci_file_is_a_problem(self):
        root = make_tree(self)
        (root / "scripts/e2e/probe.sh").write_text("true\n", encoding="utf-8")
        self.assertEqual(em.check(root), ["file selector scripts/e2e/probe.sh: no rule matches"])

    def test_rule_naming_a_missing_step_is_a_problem(self):
        root = make_tree(self, RULES.replace('step = "Demo tests" }]\n\n[[rule]]\nkind = "cargo"\nmatch = "demo/test/soak"',
                                             'step = "Absent step" }]\n\n[[rule]]\nkind = "cargo"\nmatch = "demo/test/soak"'))
        self.assertEqual(em.check(root), ["rule 3 (cargo demo/lib): ci.yml job linux has no step named 'Absent step'"])

    def test_rule_naming_a_missing_job_is_a_problem(self):
        root = make_tree(self, RULES.replace('job = "offline", step', 'job = "gone", step'))
        self.assertEqual(em.check(root), ["rule 1 (offline-check viewer/*): ci.yml has no job 'gone'"])

    def test_rule_matching_nothing_is_a_problem(self):
        shadowed = RULES.replace('[[rule]]\nkind = "cargo"\nmatch = "demo/test/soak"',
                                 '[[rule]]\nkind = "cargo"\nmatch = "demo/lib"\nmode = "synthetic"\ntier = "offline"\n'
                                 'cadence = "every-push"\nruns = []\n\n[[rule]]\nkind = "cargo"\nmatch = "demo/test/soak"')
        self.assertEqual(em.check(make_tree(self, shadowed)), ["rule 4 (cargo demo/lib): matches no selector"])

    def test_vocabulary_why_and_proof_rules(self):
        text = RULES.replace('tier = "hosted-live"\ncadence = "every-push"\nwhy = "needs a real server"\nruns = [{ job = "desktop", step = "Live proof", proof = "cua" }]',
                             'tier = "moon"\ncadence = "yearly"\nruns = [{ job = "desktop", step = "Live proof" }]')
        problems = em.check(make_tree(self, text))
        self.assertEqual(problems, [
            "rule 2 (e2e run-x.py/*): tier 'moon' is not in the tier vocabulary",
            "rule 2 (e2e run-x.py/*): cadence 'yearly' is not in the cadence vocabulary",
            "rule 2 (e2e run-x.py/*): a rule outside the offline tier needs a why",
            "rule 2 (e2e run-x.py/*): a run of kind e2e needs a proof",
        ])

    def test_split_package_must_be_a_crate(self):
        problems = em.check(make_tree(self, RULES.replace("split_packages = []", 'split_packages = ["ghost"]')))
        self.assertEqual(problems, ["scripts/ci/evidence-rules.toml: split_packages names ghost, which is not a crate"])

    def test_test_in_a_split_lib_root_is_a_problem(self):
        root = make_tree(self, RULES.replace("split_packages = []", 'split_packages = ["demo"]'))
        (root / "crates/demo/src/inner.rs").write_text("#[test]\nfn inner() {}\n", encoding="utf-8")
        problems = em.check(root)
        self.assertIn("cargo target: crates/demo/src/lib.rs holds a test in a lib root that is split into module groups", problems)

    def test_unsupported_check_argument_raises_with_location(self):
        root = make_tree(self)
        with open(root / "scripts/e2e/run-x.py", "a", encoding="utf-8") as handle:
            handle.write("        self.check(label)\n")
        with self.assertRaisesRegex(em.MatrixError, r"run-x\.py:6: unsupported check\(\) argument form Name"):
            em.check(root)

    def test_crlf_workflow_and_posix_paths(self):
        root = make_tree(self)
        (root / ".github/workflows/ci.yml").write_bytes(CI.replace("\n", "\r\n").encode("utf-8"))
        self.assertEqual(em.check(root), [])
        self.assertTrue(all("\\" not in selector for _, selector in em.enumerate_selectors(root, set())))


class FakeResponse(io.BytesIO):
    status = 200

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


def refused(code):
    return urllib.error.HTTPError("https://api.invalid/jobs", code, "refused", {}, io.BytesIO())


def jobs_payload(**extra):
    return json.dumps({"jobs": [
        {"name": "Linux checks", "status": "completed", "conclusion": "success",
         "started_at": "2026-01-01T00:00:00Z", "completed_at": "2026-01-01T00:01:30Z",
         "steps": [{"name": "Demo tests", "conclusion": "success"}]},
        {"name": "Offline proofs", "status": "completed", "conclusion": "success",
         "started_at": "2026-01-01T00:00:00Z", "completed_at": "2026-01-01T00:03:00Z",
         "steps": [{"name": "Offline viewer proof", "conclusion": "success"}]},
        {"name": "Live proofs", "status": "completed", "conclusion": "failure",
         "started_at": "2026-01-01T00:00:00Z", "completed_at": "2026-01-01T00:20:00Z",
         "steps": [{"name": "Live proof", "conclusion": "failure"}]},
        {"name": "Evidence matrix (report only)", "status": "in_progress", "conclusion": None, "steps": []},
    ], **extra}).encode("utf-8")


class RenderTests(unittest.TestCase):
    FACTS = {"repository": "owner/repo", "run_id": "7", "run_attempt": "1", "sha": "abc", "event": "push",
             "ref": "refs/heads/<b>x", "token": "secret-token", "caller_workflow_ref": "", "job_workflow_ref": "",
             "needs": {"linux": {"result": "success"}, "offline": {"result": "success"},
                       "desktop": {"result": "failure"}}}

    def artifacts(self, offline=True, desktop=True):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        base = Path(directory.name)
        if offline:
            target = base / "offline-proof-summaries/offline-viewer"
            target.mkdir(parents=True)
            (target / "artifact-summary.json").write_text(json.dumps(
                {"proof": "viewer", "status": "passed", "completed_checks": ["a.connected", "b.connected", "bad label"]}),
                encoding="utf-8")
        if desktop:
            target = base / "desktop-proof-evidence/cua/proof"
            target.mkdir(parents=True)
            (target / "summary.json").write_text(json.dumps(
                {"status": "failed", "unverified": ["ignored"],
                 "checks": [{"check": "a.connected", "passed": True, "secret": "do-not-read"},
                            {"check": "b.connected", "passed": True}, {"check": "viewer_started", "passed": False}]}),
                encoding="utf-8")
        return base

    def opener(self, *responses):
        calls = []
        queue = list(responses)

        def opener(request, timeout=None):
            calls.append((request.full_url, request.get_header("Authorization"), timeout))
            item = queue.pop(0)
            if isinstance(item, Exception):
                raise item
            return FakeResponse(item)
        opener.calls = calls
        return opener

    def report(self, root=None, opener=None, **changes):
        root = root or make_tree(self)
        facts = {**self.FACTS, **changes}
        return em.build_report(root, self.artifacts(), facts, opener or self.opener(jobs_payload()))

    def rows(self, report):
        return {row["id"]: row for row in report["rows"]}

    def test_toml_parses_and_rows_have_the_required_keys(self):
        root, output = make_tree(self), Path(tempfile.mkdtemp())
        self.addCleanup(lambda: shutil.rmtree(output, ignore_errors=True))
        em.render(root, self.artifacts(), output, self.FACTS, self.opener(jobs_payload()))
        document = tomllib.loads((output / "evidence-matrix.toml").read_text(encoding="utf-8"))
        self.assertEqual(document["schema"], 1)
        self.assertEqual(document["run"]["jobs_api"], "token")
        self.assertEqual(document["run"]["jobs_api_attempts"], ["token: 200"])
        self.assertTrue(document["row"])
        for row in document["row"]:
            for key in ("id", "kind", "selector", "mode", "tier", "cadence", "result", "why", "runs", "labels", "note"):
                self.assertIn(key, row)
            self.assertIn(row["result"], em.RESULTS)
            if row["tier"] != "offline":
                self.assertTrue(row["why"])

    def test_proof_rows_and_template_labels(self):
        rows = self.rows(self.report())
        self.assertEqual(rows["offline/viewer/a.connected"]["result"], "passed")
        self.assertEqual(rows["offline/viewer/c.hidden"]["result"], "not run")
        self.assertEqual(rows["offline/viewer/c.hidden"]["note"], "the proof passed without reaching this check")
        template = rows["e2e/run-x.py/{}.connected"]
        self.assertEqual((template["result"], template["labels"]), ("passed", ["a.connected", "b.connected"]))
        self.assertEqual(rows["e2e/run-x.py/go"]["result"], "failed")
        self.assertEqual(rows["e2e/run-x.py/go"]["note"], "failed or not reached")
        self.assertEqual(rows["e2e/run-x.py/viewer_started"]["result"], "failed")

    def test_template_dot_matches_only_a_dot(self):
        self.assertEqual(em.label_matches("run-x.py/{}.connected", {"a.connected", "aXconnected", "a.connected2"}),
                         ["a.connected"])

    def test_step_rows_use_step_conclusions(self):
        rows = self.rows(self.report())
        self.assertEqual(rows["cargo/demo/lib"]["result"], "passed")
        self.assertEqual(rows["cargo/demo/lib"]["note"], "")
        self.assertEqual(rows["ignored/crates/demo/tests/soak.rs::slow"]["result"], "not run")

    def test_failed_step_and_missing_proof_file(self):
        payload = json.loads(jobs_payload())
        payload["jobs"][0]["steps"][0]["conclusion"] = "failure"
        report = em.build_report(make_tree(self), self.artifacts(offline=False, desktop=False), self.FACTS,
                                 self.opener(json.dumps(payload).encode("utf-8")))
        rows = self.rows(report)
        self.assertEqual(rows["cargo/demo/lib"]["result"], "failed")
        self.assertEqual(rows["offline/viewer/a.connected"]["result"], "not run")
        self.assertEqual(rows["e2e/run-x.py/go"]["result"], "failed")

    def test_skipped_step_gives_skipped_rows(self):
        payload = json.loads(jobs_payload())
        payload["jobs"][2]["steps"][0]["conclusion"] = "skipped"
        report = em.build_report(make_tree(self), self.artifacts(offline=False, desktop=False), self.FACTS,
                                 self.opener(json.dumps(payload).encode("utf-8")))
        self.assertEqual(self.rows(report)["e2e/run-x.py/go"]["result"], "skipped")

    def test_durations_and_budgets(self):
        report = self.report()
        jobs = {job["id"]: job for job in report["jobs"]}
        self.assertEqual((jobs["linux"]["minutes"], jobs["linux"]["budget_minutes"]), (1.5, 20))
        self.assertEqual((jobs["desktop"]["minutes"], jobs["desktop"]["budget_minutes"]), (20.0, 90))
        self.assertNotIn("evidence-matrix", jobs)
        self.assertNotIn("budget_minutes", jobs["success"])
        self.assertEqual(report["budget"]["offline_longest_minutes"], 3.0)

    def test_anonymous_read_after_a_refused_token(self):
        opener = self.opener(refused(403), jobs_payload())
        report = self.report(opener=opener)
        self.assertEqual((report["run"]["jobs_api"], report["run"]["jobs_api_attempts"]),
                         ("anonymous", ["token: 403", "anonymous: 200"]))
        self.assertEqual([call[1] for call in opener.calls], ["Bearer secret-token", None])
        self.assertTrue(all(call[0].endswith("/repos/owner/repo/actions/runs/7/attempts/1/jobs?per_page=100")
                            for call in opener.calls))
        self.assertTrue(all(call[2] == 30 for call in opener.calls))

    def test_unavailable_api_falls_back_to_needs(self):
        opener = self.opener(refused(404), refused(404))
        report = self.report(opener=opener)
        self.assertEqual(report["run"]["jobs_api"], "unavailable: token: 404, anonymous: 404")
        row = self.rows(report)["cargo/demo/lib"]
        self.assertEqual((row["result"], row["note"]), ("passed", "job result; step conclusions unavailable"))
        self.assertEqual(self.rows(report)["e2e/run-x.py/go"]["result"], "failed")
        self.assertTrue(all("minutes" not in job for job in report["jobs"]))

    def test_called_workflow_skips_the_api(self):
        opener = self.opener()
        report = self.report(opener=opener, caller_workflow_ref="o/r/.github/workflows/release.yml@refs/tags/v1",
                             job_workflow_ref="o/r/.github/workflows/ci.yml@refs/tags/v1")
        self.assertEqual(report["run"]["jobs_api"], "skipped: called workflow")
        self.assertEqual(opener.calls, [])
        self.assertEqual(self.rows(report)["cargo/demo/lib"]["note"], "job result; step conclusions unavailable")

    def test_same_workflow_ref_reads_the_api(self):
        same = "o/r/.github/workflows/ci.yml@refs/heads/main"
        self.assertEqual(self.report(caller_workflow_ref=same, job_workflow_ref=same)["run"]["jobs_api"], "token")

    def test_missing_run_identity_is_unavailable_without_a_call(self):
        opener = self.opener()
        report = self.report(opener=opener, repository="", run_id="")
        self.assertEqual(report["run"]["jobs_api"], "unavailable: run identity missing")
        self.assertEqual(opener.calls, [])

    def test_html_has_no_external_resource_and_escapes_values(self):
        output = Path(tempfile.mkdtemp())
        self.addCleanup(lambda: shutil.rmtree(output, ignore_errors=True))
        em.render(make_tree(self), self.artifacts(), output, self.FACTS, self.opener(jobs_payload()))
        page = (output / "evidence-matrix.html").read_text(encoding="utf-8")
        for forbidden in ("<script", "<link", "src=", "url(", "http", "@import"):
            self.assertNotIn(forbidden, page)
        self.assertIn("refs/heads/&lt;b&gt;x", page)
        self.assertNotIn("<b>x", page)
        self.assertNotIn("secret-token", page)
        self.assertIsNone(re.search(r"#[0-9]{3,}", page))
        self.assertIn("failed or not reached", page)

    def test_summary_is_appended(self):
        output, summary = Path(tempfile.mkdtemp()), Path(tempfile.mkdtemp()) / "summary.md"
        self.addCleanup(lambda: shutil.rmtree(output, ignore_errors=True))
        summary.write_text("earlier\n", encoding="utf-8")
        em.render(make_tree(self), self.artifacts(), output, self.FACTS, self.opener(jobs_payload()), str(summary))
        text = summary.read_text(encoding="utf-8")
        self.assertTrue(text.startswith("earlier\n"))
        self.assertIn("Jobs API: token", text)
        self.assertIn("| offline |", text)

    def test_mapping_problem_is_a_generator_error(self):
        root = make_tree(self)
        (root / "scripts/e2e/probe.sh").write_text("true\n", encoding="utf-8")
        with self.assertRaisesRegex(em.MatrixError, "probe.sh: no rule matches"):
            self.report(root=root)

    def test_real_tree_renders_without_artifacts_or_network(self):
        output = Path(tempfile.mkdtemp())
        self.addCleanup(lambda: shutil.rmtree(output, ignore_errors=True))
        facts = {"needs": {}}
        refuse = self.opener()
        report = em.render(ROOT, output / "none", output / "out", facts, refuse)
        self.assertEqual(refuse.calls, [])
        self.assertTrue(report["rows"])
        document = tomllib.loads((output / "out/evidence-matrix.toml").read_text(encoding="utf-8"))
        self.assertEqual(len(document["row"]), len(report["rows"]))
        ignored = {row["selector"]: row for row in document["row"] if row["kind"] == "ignored"}
        self.assertEqual({row["result"] for s, row in ignored.items() if "live_daemon_windows_dacl" not in s}, {"not run"})


if __name__ == "__main__":
    unittest.main()
