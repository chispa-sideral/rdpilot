#!/usr/bin/env python3
"""Require every named CI predecessor to succeed, including on skipped jobs."""
import json
import os
import sys


def main():
    expected = sys.argv[1:]
    try:
        results = json.loads(os.environ["NEEDS_JSON"])
    except (KeyError, ValueError):
        print("Required CI results are unavailable or invalid.")
        return 1
    if not expected or len(set(expected)) != len(expected) or not isinstance(results, dict):
        print("Required CI result names are invalid.")
        return 1
    if set(results) != set(expected):
        print("Required CI predecessors are missing or unexpected.")
        return 1
    if any(not isinstance(results[name], dict) or results[name].get("result") != "success"
           for name in expected):
        print("Every required CI predecessor must succeed.")
        return 1
    print("All required CI predecessors succeeded.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
