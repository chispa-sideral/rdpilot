#!/usr/bin/env python3
"""Map every assertion of the repository to a tier and render the evidence matrix of a CI run.

`check` enumerates the selectors in the tree (offline proof checks, live check() sites,
Cargo test targets, ignored tests and CI test files) and matches each one against the
pattern rules in scripts/ci/evidence-rules.toml. It reads files only and runs no cargo
command. `render` joins the rows with the results of one run and writes
evidence-matrix.toml and evidence-matrix.html. The repository holds no run data.
"""
import argparse
import ast
from datetime import datetime
import fnmatch
import html
import importlib.util
import json
import os
from pathlib import Path
import re
import sys
import tomllib
import urllib.error
import urllib.request


ROOT = Path(__file__).resolve().parents[2]
RULES = "scripts/ci/evidence-rules.toml"
WORKFLOW = ".github/workflows/ci.yml"
REPORT_JOB = "evidence-matrix"
KINDS = {"offline-check": "offline", "e2e": "e2e", "cargo": "cargo", "ignored": "ignored", "file": "file"}
PROOF_KINDS = ("offline-check", "e2e")
RESULTS = ("passed", "failed", "skipped", "not run")
# The e2e artifacts hold disjoint proof names, one directory per proof.
ARTIFACTS = {"offline-check": "offline-proof-summaries",
             "e2e": ("desktop-proof-evidence", "desktop-recording-proof-evidence")}
LABEL = re.compile(r"[A-Za-z0-9_.\-]{1,200}")
TEST_ATTRIBUTE = re.compile(r"^\s*#\[(?:tokio::)?test\b", re.MULTILINE)
IGNORE_ATTRIBUTE = re.compile(r"^\s*#\[(?:ignore\b|cfg_attr\(.*\bignore\b)", re.MULTILINE)
MODULE_DECLARATION = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*;", re.MULTILINE)
FUNCTION = re.compile(r"\bfn\s+(\w+)")
JOB_HEADER = re.compile(r"^  ([a-z][a-z0-9-]*):\s*$")
JOB_NAME = re.compile(r"^    name:\s*(.+?)\s*$")
STEP_NAME = re.compile(r"^      - name:\s*(.+?)\s*$")
API = "https://api.github.com/repos/{repository}/actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100"


class MatrixError(Exception):
    """A source file or the rules have a form that the generator does not support."""


def read(root, relative):
    return (Path(root) / relative).read_text(encoding="utf-8")


def posix(root, path):
    return path.relative_to(root).as_posix()


# --- enumeration (functions of a repository root) ---------------------------------


def offline_checks(root):
    """The (proof, label) pairs of CHECKS in scripts/ci/run-offline-proof.py."""
    path = Path(root) / "scripts/ci/run-offline-proof.py"
    if not path.exists():
        return set()
    spec = importlib.util.spec_from_file_location("evidence_offline_proof", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return {(proof, label) for proof, labels in module.CHECKS.items() for label in labels}


def site_names(node, where):
    """The check() site strings of one first argument; a placeholder becomes {}."""
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return [node.value]
    if isinstance(node, ast.JoinedStr):
        return ["".join(v.value if isinstance(v, ast.Constant) else "{}" for v in node.values)]
    if (isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add)
            and isinstance(node.left, ast.Constant) and isinstance(node.left.value, str)
            and isinstance(node.right, ast.IfExp)
            and isinstance(node.right.body, ast.Constant) and isinstance(node.right.orelse, ast.Constant)
            and isinstance(node.right.body.value, str) and isinstance(node.right.orelse.value, str)):
        return [node.left.value + node.right.body.value, node.left.value + node.right.orelse.value]
    raise MatrixError(f"{where}: unsupported check() argument form {type(node).__name__}")


def check_sites(root):
    """Every check() site of scripts/e2e/*.py as {(file name, site): [file:line]}."""
    sites = {}
    for path in sorted((Path(root) / "scripts/e2e").glob("*.py")):
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=path.name)
        for node in ast.walk(tree):
            if not (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                    and node.func.attr == "check"):
                continue
            where = f"{path.name}:{node.lineno}"
            if len(node.args) < 1:
                raise MatrixError(f"{where}: check() without a positional label")
            for site in site_names(node.args[0], where):
                sites.setdefault((path.name, site), []).append(where)
    return sites


def has_test(path):
    return bool(TEST_ATTRIBUTE.search(path.read_text(encoding="utf-8")))


def rust_files(directory):
    return sorted(directory.rglob("*.rs")) if directory.is_dir() else []


def lib_modules(crate_dir):
    """Top-level modules of a library (file stem or directory) whose files hold a test."""
    src = crate_dir / "src"
    groups = set()
    for path in rust_files(src):
        parts = path.relative_to(src).parts
        if parts in (("lib.rs",), ("main.rs",)) or parts[0] == "bin":
            continue
        if has_test(path):
            groups.add(parts[0] if len(parts) > 1 else path.stem)
    return groups


def crate_targets(crate_dir):
    """Test targets of one crate: (package, has lib, {bin name: path}, test names)."""
    manifest = tomllib.loads((crate_dir / "Cargo.toml").read_text(encoding="utf-8"))
    package = manifest["package"]["name"]
    has_lib = "lib" in manifest or (crate_dir / "src/lib.rs").exists()
    bins = {}
    for entry in manifest.get("bin", []):
        name = entry["name"]
        bins[name] = entry.get("path") or ("src/main.rs" if name == package else f"src/bin/{name}.rs")
    if (crate_dir / "src/main.rs").exists() and "src/main.rs" not in bins.values():
        bins[package] = "src/main.rs"
    for path in sorted((crate_dir / "src/bin").glob("*.rs")):
        bins.setdefault(path.stem, f"src/bin/{path.name}")
    for path in sorted((crate_dir / "src/bin").glob("*/main.rs")):
        bins.setdefault(path.parent.name, f"src/bin/{path.parent.name}/main.rs")
    tests = {path.stem for path in (crate_dir / "tests").glob("*.rs")}
    tests |= {entry["name"] for entry in manifest.get("test", [])}
    return package, has_lib, bins, sorted(tests)


def bin_needs_row(crate_dir, relative):
    """A bin root without a module declaration and without a test has nothing to prove."""
    text = (crate_dir / relative).read_text(encoding="utf-8")
    return bool(MODULE_DECLARATION.search(text) or TEST_ATTRIBUTE.search(text))


def cargo_required(root, split_packages):
    """The cargo selectors of the tree, the lib roots that hold a test, and the package names.

    A package in split_packages has its lib folded into one selector per module group.
    """
    required, lib_root_tests, packages = set(), [], set()
    for manifest in sorted((Path(root) / "crates").glob("*/Cargo.toml")):
        crate_dir = manifest.parent
        package, has_lib, bins, tests = crate_targets(crate_dir)
        packages.add(package)
        if has_lib:
            if package in split_packages:
                required |= {f"{package}/lib/{group}" for group in lib_modules(crate_dir)}
                if (crate_dir / "src/lib.rs").exists() and has_test(crate_dir / "src/lib.rs"):
                    lib_root_tests.append(posix(Path(root), crate_dir / "src/lib.rs"))
            else:
                required.add(f"{package}/lib")
        for name, relative in bins.items():
            if bin_needs_row(crate_dir, relative):
                required.add(f"{package}/bin/{name}")
        required |= {f"{package}/test/{name}" for name in tests}
    return required, lib_root_tests, packages


def ignored_tests(root):
    """The <repo path>::<fn name> of every #[ignore] attribute under crates/."""
    root = Path(root)
    found = set()
    for path in rust_files(root / "crates"):
        text = path.read_text(encoding="utf-8")
        for match in IGNORE_ATTRIBUTE.finditer(text):
            name = FUNCTION.search(text, match.end())
            where = f"{posix(root, path)}:{text.count(chr(10), 0, match.start()) + 1}"
            if not name:
                raise MatrixError(f"{where}: ignore attribute without a following fn")
            found.add(f"{posix(root, path)}::{name.group(1)}")
    return found


def ci_files(root):
    root = Path(root)
    found = {posix(root, p) for p in (root / "scripts/ci").glob("test_*.py")}
    return found | {posix(root, p) for p in (root / "scripts/e2e").glob("*.sh")}


def enumerate_selectors(root, split_packages):
    """Every selector of the tree as {(kind, selector): where}; where is a file:line list or ''."""
    found = {("offline-check", f"{proof}/{label}"): "" for proof, label in offline_checks(root)}
    for (file, site), where in check_sites(root).items():
        found[("e2e", f"{file}/{site}")] = ", ".join(where)
    required, _, _ = cargo_required(root, split_packages)
    found.update({("cargo", item): "" for item in required})
    found.update({("ignored", item): "" for item in ignored_tests(root)})
    found.update({("file", item): "" for item in ci_files(root)})
    return found


# --- rules and workflow ------------------------------------------------------------


def unquote(value):
    return value[1:-1] if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"" else value


def workflow_jobs(root):
    """{job id: {"name": job name, "steps": [step names]}} read from fixed indentation."""
    text = read(root, WORKFLOW)
    if "\njobs:\n" not in text:
        raise MatrixError(f"{WORKFLOW}: no jobs section")
    jobs, current = {}, None
    for line in text.split("\njobs:\n", 1)[1].splitlines():
        if line and not line.startswith(" "):
            break
        if match := JOB_HEADER.match(line):
            current = jobs[match[1]] = {"name": match[1], "steps": []}
        elif current is not None and (match := JOB_NAME.match(line)):
            current["name"] = unquote(match[1])
        elif current is not None and (match := STEP_NAME.match(line)):
            current["steps"].append(unquote(match[1]))
    return jobs


def load_rules(root):
    try:
        return tomllib.loads(read(root, RULES))
    except tomllib.TOMLDecodeError as error:
        raise MatrixError(f"{RULES}: {error}") from error


def find_rule(rules, kind, selector):
    """The index of the first rule of this kind whose glob matches the selector."""
    for index, rule in enumerate(rules.get("rule", [])):
        if rule.get("kind") == kind and fnmatch.fnmatchcase(selector, str(rule.get("match", ""))):
            return index
    return None


def rule_label(index, rule):
    return f"rule {index + 1} ({rule.get('kind')} {rule.get('match')})"


def rule_problems(rules, jobs, packages):
    problems = []
    if rules.get("schema") != 1:
        problems.append(f"{RULES}: schema must be 1")
    tables = (("tier", "tiers"), ("mode", "modes"), ("cadence", "cadences"))
    vocabularies = {key: rules.get(table, {}) for key, table in tables}
    for key, table in tables:
        if not vocabularies[key] or not all(isinstance(v, str) and v.strip() for v in vocabularies[key].values()):
            problems.append(f"{RULES}: [{table}] needs a non-empty definition per entry")
    for package in rules.get("split_packages", []):
        if package not in packages:
            problems.append(f"{RULES}: split_packages names {package}, which is not a crate")
    budget = rules.get("budget", {})
    for key in ("offline_jobs", "desktop_jobs"):
        problems += [f"{RULES}: budget {key} names {job}, which is not a ci.yml job"
                     for job in budget.get(key, []) if job not in jobs]
    for index, rule in enumerate(rules.get("rule", [])):
        label = rule_label(index, rule)
        if rule.get("kind") not in KINDS:
            problems.append(f"{label}: kind must be one of {', '.join(KINDS)}")
        if not str(rule.get("match", "")).strip():
            problems.append(f"{label}: match is empty")
        for key, vocabulary in vocabularies.items():
            if rule.get(key) not in vocabulary:
                problems.append(f"{label}: {key} {rule.get(key)!r} is not in the {key} vocabulary")
        if rule.get("tier") != "offline" and not str(rule.get("why", "")).strip():
            problems.append(f"{label}: a rule outside the offline tier needs a why")
        for run in rule.get("runs", []):
            job, step = run.get("job"), run.get("step")
            if job not in jobs:
                problems.append(f"{label}: ci.yml has no job {job!r}")
            elif step not in jobs[job]["steps"]:
                problems.append(f"{label}: ci.yml job {job} has no step named {step!r}")
            if rule.get("kind") in PROOF_KINDS and not str(run.get("proof", "")).strip():
                problems.append(f"{label}: a run of kind {rule.get('kind')} needs a proof")
    return problems


def check(root):
    """Every mapping problem of the tree at root, one line each; empty when the tree is covered."""
    rules, jobs = load_rules(root), workflow_jobs(root)
    split = set(rules.get("split_packages", []))
    _, lib_root_tests, packages = cargo_required(root, split)
    problems = rule_problems(rules, jobs, packages)
    problems += [f"cargo target: {name} holds a test in a lib root that is split into module groups"
                 for name in lib_root_tests]
    used = set()
    for (kind, selector), where in sorted(enumerate_selectors(root, split).items()):
        index = find_rule(rules, kind, selector)
        if index is None:
            problems.append(f"{kind} selector {selector}: no rule matches" + (f" ({where})" if where else ""))
        used.add(index)
    problems += [f"{rule_label(i, rule)}: matches no selector"
                 for i, rule in enumerate(rules.get("rule", [])) if i not in used]
    return problems


# --- results -----------------------------------------------------------------------


def clean(value):
    """A printable ASCII copy of an environment value."""
    return re.sub(r"[^\x20-\x7e]", "?", str(value))


def read_json(path):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def labels_of(names):
    return {name for name in (names if isinstance(names, list) else []) if isinstance(name, str) and LABEL.fullmatch(name)}


def proof_results(artifacts):
    """{kind: {proof: (passed, labels)}} from the downloaded artifacts of the run.

    Only the status and the names of passed checks are read, never a data field.
    """
    found = {"offline-check": {}, "e2e": {}}
    base = Path(artifacts) / ARTIFACTS["offline-check"]
    for path in sorted(base.rglob("artifact-summary.json")):
        data = read_json(path)
        if isinstance(data, dict) and isinstance(data.get("proof"), str):
            found["offline-check"][data["proof"]] = (
                data.get("status") == "passed", labels_of(data.get("completed_checks")))
    paths = [path for name in ARTIFACTS["e2e"] for path in sorted((Path(artifacts) / name).rglob("summary.json"))]
    for path in paths:
        data = read_json(path)
        if path.parent.name == "proof" and isinstance(data, dict):
            checks = data.get("checks")
            names = [c.get("check") for c in checks if isinstance(c, dict) and c.get("passed") is True] \
                if isinstance(checks, list) else []
            found["e2e"][path.parent.parent.name] = (data.get("status") == "passed", labels_of(names))
    return found


def label_matches(selector, labels):
    """The labels that the check site of this selector produces; {} stands for [A-Za-z0-9_]+."""
    site = selector.split("/", 1)[1]
    pattern = re.compile("[A-Za-z0-9_]+".join(re.escape(part) for part in site.split("{}")))
    return sorted(label for label in labels if pattern.fullmatch(label))


def step_state(job, step, steps, needs):
    """(ok | skipped | failed | None, whether the step conclusion came from the jobs API)."""
    conclusion = steps.get(job, {}).get(step)
    if conclusion is not None:
        return {"success": "ok", "skipped": "skipped"}.get(conclusion, "failed"), True
    if job in steps:
        return None, True
    result = needs.get(job, {}).get("result") if isinstance(needs.get(job), dict) else None
    return {"success": "ok", "skipped": "skipped", "failure": "failed", "cancelled": "failed"}.get(result), False


def proof_row_result(row, rule, proofs, state):
    """(result, note, labels) of an offline-check or e2e row."""
    found, failed, clean_pass = set(), False, False
    for run in rule["runs"]:
        entry = proofs[row["kind"]].get(run["proof"])
        if entry is None:
            continue
        passed, labels = entry
        hits = label_matches(row["selector"], labels)
        if hits:
            found.update(hits)
        elif passed:
            clean_pass = True
        else:
            failed = True
    if found:
        return "passed", "", sorted(found)
    if failed:
        return "failed", "failed or not reached", []
    if clean_pass:
        return "not run", "the proof passed without reaching this check", []
    states = [state(run["job"], run["step"])[0] for run in rule["runs"]]
    if "failed" in states:
        return "failed", "a listed step failed and the proof wrote no result", []
    if all(s == "skipped" for s in states):
        return "skipped", "the listed step was skipped", []
    return "not run", "no proof result", []


def step_row_result(rule, state):
    """(result, note) of a cargo, ignored or file row from the conclusions of its steps."""
    pairs = [state(run["job"], run["step"]) for run in rule["runs"]]
    states = [s for s, _ in pairs]
    note = "job result; step conclusions unavailable" if any(not api for _, api in pairs) else ""
    if "failed" in states:
        return "failed", note
    if None in states:
        return "not run", note or "no step result"
    if "skipped" in states:
        return "skipped", note
    return "passed", note


def build_rows(root, rules, proofs, steps, needs):
    state = lambda job, step: step_state(job, step, steps, needs)
    rows = []
    order = {kind: index for index, kind in enumerate(KINDS)}
    selectors = enumerate_selectors(root, set(rules.get("split_packages", [])))
    for kind, selector in sorted(selectors, key=lambda item: (order[item[0]], item[1])):
        rule = rules["rule"][find_rule(rules, kind, selector)]
        row = {"id": f"{KINDS[kind]}/{selector}", "kind": kind, "selector": selector, "mode": rule["mode"],
               "tier": rule["tier"], "cadence": rule["cadence"], "why": rule.get("why", ""),
               "runs": [f"{run['job']}: {run['step']}" for run in rule["runs"]], "labels": []}
        if not rule["runs"]:
            row["result"], row["note"] = "not run", rule.get("why") or "no CI step runs it"
        elif kind in PROOF_KINDS:
            row["result"], row["note"], row["labels"] = proof_row_result(row, rule, proofs, state)
        else:
            row["result"], row["note"] = step_row_result(rule, state)
        rows.append(row)
    return rows


# --- jobs API ----------------------------------------------------------------------


def fetch_jobs(facts, opener=urllib.request.urlopen):
    """(source, attempts, jobs): the jobs of this run attempt from the REST API.

    The read tries the job token, then no Authorization header, and gives up with a
    reason. Under workflow_call the API is not read.
    """
    caller, own = facts.get("caller_workflow_ref", ""), facts.get("job_workflow_ref", "")
    if caller and own and caller != own:
        return "skipped: called workflow", [], None
    if not all(facts.get(key) for key in ("repository", "run_id", "run_attempt")):
        return "unavailable: run identity missing", [], None
    url = API.format(repository=facts["repository"], run_id=facts["run_id"], attempt=facts["run_attempt"])
    attempts = []
    for source in ("token", "anonymous"):
        if source == "token" and not facts.get("token"):
            continue
        headers = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28",
                   "User-Agent": "rdpilot-evidence-matrix"}
        if source == "token":
            headers["Authorization"] = f"Bearer {facts['token']}"
        try:
            with opener(urllib.request.Request(url, headers=headers), timeout=30) as response:
                body = json.load(response)
            jobs = body.get("jobs") if isinstance(body, dict) else None
            if isinstance(jobs, list):
                attempts.append(f"{source}: {getattr(response, 'status', 200)}")
                return source, attempts, jobs
            attempts.append(f"{source}: unexpected body")
        except urllib.error.HTTPError as error:
            attempts.append(f"{source}: {error.code}")
        except (OSError, ValueError) as error:
            attempts.append(f"{source}: {type(error).__name__}")
    return "unavailable: " + ", ".join(attempts or ["no attempt"]), attempts, None


def minutes(job):
    try:
        start = datetime.fromisoformat(job["started_at"])
        end = datetime.fromisoformat(job["completed_at"])
    except (KeyError, TypeError, ValueError):
        return None
    return round((end - start).total_seconds() / 60, 1)


def api_jobs(workflow, api):
    """({job id: API job}, {job id: {step name: conclusion}}) for completed jobs named as in ci.yml."""
    by_name = {job["name"]: job_id for job_id, job in workflow.items() if job_id != REPORT_JOB}
    jobs, steps = {}, {}
    for job in api or []:
        job_id = by_name.get(job.get("name")) if isinstance(job, dict) else None
        if job_id is None or job.get("status") != "completed":
            continue
        jobs[job_id] = job
        steps[job_id] = {s["name"]: s.get("conclusion") for s in job.get("steps", [])
                         if isinstance(s, dict) and isinstance(s.get("name"), str)}
    return jobs, steps


# --- report ------------------------------------------------------------------------


def facts_from_environ(environ):
    names = {"repository": "GITHUB_REPOSITORY", "run_id": "GITHUB_RUN_ID", "run_attempt": "GITHUB_RUN_ATTEMPT",
             "sha": "GITHUB_SHA", "event": "GITHUB_EVENT_NAME", "ref": "GITHUB_REF", "token": "GITHUB_TOKEN",
             "caller_workflow_ref": "CALLER_WORKFLOW_REF", "job_workflow_ref": "JOB_WORKFLOW_REF"}
    facts = {key: environ.get(name, "") for key, name in names.items()}
    try:
        needs = json.loads(environ.get("NEEDS_JSON", "{}"))
    except ValueError:
        needs = {}
    facts["needs"] = needs if isinstance(needs, dict) else {}
    return facts


def build_report(root, artifacts, facts, opener=urllib.request.urlopen):
    rules, workflow = load_rules(root), workflow_jobs(root)
    problems = check(root)
    if problems:
        raise MatrixError("mapping problems:\n" + "\n".join(problems))
    source, attempts, api = fetch_jobs(facts, opener)
    jobs, steps = api_jobs(workflow, api)
    if api is not None and not jobs:
        source = "unavailable: no job of the API matches ci.yml"
    needs = facts.get("needs", {})
    rows = build_rows(root, rules, proof_results(artifacts), steps, needs)
    budget = rules.get("budget", {})
    limits = {job: budget.get("offline_minutes") for job in budget.get("offline_jobs", [])}
    limits.update({job: budget.get("desktop_minutes") for job in budget.get("desktop_jobs", [])})
    job_rows = []
    for job_id, job in workflow.items():
        if job_id == REPORT_JOB:
            continue
        api_job = jobs.get(job_id)
        result = (needs.get(job_id) or {}).get("result") if isinstance(needs.get(job_id), dict) else None
        entry = {"id": job_id, "name": job["name"], "conclusion": (api_job or {}).get("conclusion") or result or "unknown"}
        if api_job and minutes(api_job) is not None:
            entry["minutes"] = minutes(api_job)
        if limits.get(job_id) is not None:
            entry["budget_minutes"] = limits[job_id]
        job_rows.append(entry)
    offline = [j["minutes"] for j in job_rows if j["id"] in budget.get("offline_jobs", []) and "minutes" in j]
    run = {"repository": facts.get("repository", ""), "sha": facts.get("sha", ""), "run_id": facts.get("run_id", ""),
           "run_attempt": facts.get("run_attempt", ""), "event": facts.get("event", ""), "ref": facts.get("ref", ""),
           "jobs_api": source, "jobs_api_attempts": attempts}
    return {"run": {key: clean(value) if isinstance(value, str) else value for key, value in run.items()},
            "budget": {"offline_minutes": budget.get("offline_minutes"), "desktop_minutes": budget.get("desktop_minutes"),
                       **({"offline_longest_minutes": max(offline)} if offline else {})},
            "jobs": job_rows, "rows": rows,
            "definitions": {key: rules.get(key, {}) for key in ("tiers", "modes", "cadences")},
            "placement": rules.get("placement", [])}


# --- output ------------------------------------------------------------------------


def toml_value(value):
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return repr(value)
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(item) for item in value) + "]"
    return json.dumps(str(value), ensure_ascii=False).replace("\x7f", "\\u007f")


def toml_table(header, values):
    return [header] + [f"{key} = {toml_value(value)}" for key, value in values.items() if value is not None] + [""]


def render_toml(report):
    lines = ["schema = 1", ""]
    lines += toml_table("[run]", report["run"])
    lines += toml_table("[budget]", report["budget"])
    for job in report["jobs"]:
        lines += toml_table("[[job]]", job)
    for row in report["rows"]:
        lines += toml_table("[[row]]", row)
    return "\n".join(lines)


CSS = """
body { font-family: system-ui, sans-serif; margin: 2rem; line-height: 1.4; color: black; background: white; }
h1, h2 { margin-bottom: 0.3rem; }
table { border-collapse: collapse; margin: 0.5rem 0 1.5rem; }
th, td { border: 1px solid gray; padding: 0.2rem 0.5rem; text-align: left; vertical-align: top; }
th { background: whitesmoke; }
td.passed { background: honeydew; }
td.failed { background: mistyrose; }
td.skipped { background: lightyellow; }
td.not-run { background: lightgray; }
"""


def h(value):
    return html.escape(str(value), quote=True)


class Raw(str):
    """A table cell that is already HTML."""


def table(headers, rows):
    head = "".join(f"<th>{h(name)}</th>" for name in headers)
    body = "".join("<tr>" + "".join(cell if isinstance(cell, Raw) else f"<td>{h(cell)}</td>" for cell in row) + "</tr>"
                   for row in rows)
    return f"<table><tr>{head}</tr>{body}</table>"


def result_cell(result):
    return Raw(f'<td class="{h(result.replace(" ", "-"))}">{h(result)}</td>')


def duration_text(report, job):
    if "minutes" not in job:
        return "unavailable"
    limit = job.get("budget_minutes")
    return f"{job['minutes']}" + ("" if limit is None else (" (over)" if job["minutes"] > limit else " (within)"))


def render_html(report):
    run = report["run"]
    parts = ["<!doctype html>", '<html lang="en"><head><meta charset="utf-8">',
             "<title>Evidence matrix</title>", f"<style>{CSS}</style></head><body>", "<h1>Evidence matrix</h1>",
             "<h2>Run</h2>",
             table(["fact", "value"], [[key, ", ".join(value) if isinstance(value, list) else value]
                                       for key, value in run.items()]),
             "<h2>Definitions</h2>"]
    for title, key in (("Tiers", "tiers"), ("Modes", "modes"), ("Cadences", "cadences")):
        parts += [f"<h3>{h(title)}</h3>", table(["name", "definition"], list(report["definitions"][key].items()))]
    parts += ["<h3>Placement rules</h3><ol>" + "".join(f"<li>{h(rule)}</li>" for rule in report["placement"]) + "</ol>",
              "<h2>Durations</h2>",
              table(["job", "name", "conclusion", "minutes", "budget minutes"],
                    [[j["id"], j["name"], j["conclusion"], duration_text(report, j), j.get("budget_minutes", "")]
                     for j in report["jobs"]]),
              "<p>The durations and the step results come from the jobs API of this run; "
              "the jobs_api fact says whether that read worked. Durations are reported, not enforced.</p>",
              "<h2>Rows</h2>",
              "<p>A result of failed with the note &quot;failed or not reached&quot; means that the proof wrote no passed "
              "entry for this check: the check failed, or the proof stopped before it.</p>"]
    for tier in report["definitions"]["tiers"]:
        rows = [r for r in report["rows"] if r["tier"] == tier]
        parts += [f"<h3>{h(tier)} ({len(rows)})</h3>",
                  table(["result", "id", "mode", "cadence", "runs", "note", "why", "labels"],
                        [[result_cell(r["result"]), r["id"], r["mode"], r["cadence"], "; ".join(r["runs"]), r["note"],
                          r["why"], ", ".join(r["labels"])] for r in rows])]
    parts.append("</body></html>")
    return "\n".join(parts)


def render_summary(report):
    tiers = list(report["definitions"]["tiers"])
    lines = ["## Evidence matrix", "", f"Jobs API: {report['run']['jobs_api']}", "",
             "| tier | " + " | ".join(RESULTS) + " |", "|---|" + "---|" * len(RESULTS)]
    for tier in tiers:
        counts = [sum(1 for r in report["rows"] if r["tier"] == tier and r["result"] == name) for name in RESULTS]
        lines.append(f"| {tier} | " + " | ".join(map(str, counts)) + " |")
    lines += ["", "| job | minutes | budget minutes |", "|---|---|---|"]
    lines += [f"| {j['id']} | {j.get('minutes', 'unavailable')} | {j.get('budget_minutes', '')} |" for j in report["jobs"]]
    return "\n".join(lines) + "\n"


def render(root, artifacts, output, facts, opener=urllib.request.urlopen, summary_path=None):
    report = build_report(root, artifacts, facts, opener)
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    (output / "evidence-matrix.toml").write_text(render_toml(report), encoding="utf-8")
    (output / "evidence-matrix.html").write_text(render_html(report), encoding="utf-8")
    if summary_path:
        with open(summary_path, "a", encoding="utf-8") as handle:
            handle.write(render_summary(report))
    return report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check")
    draw = commands.add_parser("render")
    draw.add_argument("--artifacts", required=True)
    draw.add_argument("--output", required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "check":
            problems = check(ROOT)
            print("\n".join(problems) if problems else "Every selector has a rule.")
            return 1 if problems else 0
        report = render(ROOT, args.artifacts, args.output, facts_from_environ(os.environ),
                        summary_path=os.environ.get("GITHUB_STEP_SUMMARY"))
        print(f"Evidence matrix: {len(report['rows'])} rows; jobs API: {report['run']['jobs_api']}.")
        return 0
    except (MatrixError, OSError, tomllib.TOMLDecodeError) as error:
        print(f"Evidence matrix generator error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
