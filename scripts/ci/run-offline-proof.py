#!/usr/bin/env python3
"""Run an existing fake proof and emit only a safe diagnostic projection."""
import argparse
import json
from pathlib import Path
import subprocess
import sys


# These names are trusted diagnostic labels, not strings copied from failures,
# tool results, session state or raw proof summaries.
CHECKS = {
    "viewer": (
        "a.connected", "b.connected", "c.connected", "d.connected", "viewer_started",
        "session_list_and_sessions_viewed", "strip_rows_in_order_within_budget",
        "panel_close_and_reopen_keeps_strip_and_frames", "events_route_immediate_and_per_session",
        "a.change_visible_within_budget", "a.frame_rate_capped", "a.png_size_and_encode_time",
        "a.resize_shown", "e.connected", "e.disconnect_shown", "a.close_shown_and_listed",
        "unauthorised_requests_rejected_on_every_bound_address",
        "viewer_stopped_on_ctrl_c_and_ports_closed", "evidence_contains_no_token_or_credentials",
    ),
    "recording": (
        "a.connected", "b.connected", "c.connected", "viewer_started", "recordings_listed_and_kept",
        "a_video_smaller_than_png", "recordings_on_disk", "chromium.replay", "firefox.replay",
        "viewer_stopped_on_ctrl_c_and_ports_closed", "evidence_contains_no_token_or_credentials",
    ),
    "takeover": (
        "notepad.connected", "viewer_started", "tools_list_advertises_takeover_on_acting_tools_only",
        "tab1_takeover_and_typing", "agent_refused_during_the_lease", "tab2_takeover_and_tab1_notice",
        "agent_takeover_through_mcp", "cli_takeover_as_printed", "closed_tab_returns_control",
        "status_log_and_strip_show_every_change", "viewer_stopped_on_ctrl_c_and_ports_closed",
        "viewer_started_read_only", "read_only_viewer_has_no_takeover_and_no_write_route",
        "evidence_contains_no_token_credential_lease_or_typed_text",
    ),
}


def project(proof, summary, returncode):
    checks = summary.get("checks", []) if isinstance(summary, dict) else []
    if not isinstance(checks, list):
        checks = []
    # Select constants by equality; never carry free-form values into artifacts.
    completed = [name for name in CHECKS[proof] if any(
        isinstance(check, dict) and check.get("check") == name and check.get("passed") is True
        for check in checks
    )]
    passed = (returncode == 0 and isinstance(summary, dict) and summary.get("mode") == "fake"
              and summary.get("status") == "passed" and len(completed) == len(CHECKS[proof]))
    stage = "none" if passed else ("harness" if returncode != 0 else "summary_or_evidence_scan")
    return {"proof": proof, "mode": "fake", "status": "passed" if passed else "failed",
            "completed_checks": completed, "failure_stage": stage}


def run_proof(proof, bin_dir, output):
    output = Path(output).resolve()
    if output.exists():
        raise ValueError("Proof output directory must be new.")
    script = Path(__file__).resolve().parents[1] / "e2e" / f"run-{proof}-proof.py"
    print(f"Running offline {proof} proof.", flush=True)
    # Raw exceptions can contain tool arguments, URLs or lease IDs even when a
    # proof failed before its evidence scan. They stay out of logs and artifacts.
    result = subprocess.run(
        [sys.executable, str(script), "--fake", "--bin-dir", str(Path(bin_dir).resolve()),
         "--output", str(output)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        summary = json.loads((output / "summary.json").read_text())
    except (OSError, ValueError):
        summary = None
    selected = project(proof, summary, result.returncode)
    output.mkdir(parents=True, exist_ok=True)
    (output / "artifact-summary.json").write_text(json.dumps(selected, indent=2) + "\n")
    print(f"Offline {proof} proof {selected['status']}; stage: {selected['failure_stage']}.")
    return 0 if selected["status"] == "passed" else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("proof", choices=CHECKS)
    parser.add_argument("--bin-dir", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    try:
        return run_proof(args.proof, args.bin_dir, args.output)
    except (OSError, ValueError):
        print("Offline proof could not start; no raw diagnostic artifact was selected.")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
