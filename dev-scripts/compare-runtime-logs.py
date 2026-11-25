#!/usr/bin/env python3
"""Count and compare diagnostics without losing the original log locations."""

import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path
import re


def diagnostic(line):
    lower = line.lower()
    if lower.startswith("(panic is followed") or "state immediately after panic" in lower:
        return False
    return any(word in lower for word in (
        "warning:", "panic in thread", "hyperhle errored:",
        "guest application stopped:", " is unimplemented.", "null-page read",
        "not json-serializable", "not a valid json object", "unsupported:",
        "skipping unsupported", "unavailable", "call to faked class", "todo:",
    ))


def normalize(line):
    # Pointers and rate-limit counters vary across launches. Preserve GL error
    # codes, enum values and guest receiver values: they identify different bugs.
    line = re.sub(r"\(0x[0-9a-fA-F]+\)", "(<pointer>)", line)
    line = re.sub(r" at 0x[0-9a-fA-F]+", " at <pointer>", line)
    line = re.sub(r"key 0x[0-9a-fA-F]+", "key <pointer>", line)
    line = re.sub(r"object 0x[0-9a-fA-F]+", "object <pointer>", line)
    line = re.sub(r"\[\d+/\d+\]", "[rate limited]", line)
    line = re.sub(r"unique sites logged: \d+/\d+", "unique sites logged: <rate limited>", line)
    return line


def read_log(path):
    raw = path.read_bytes()
    diagnostics = defaultdict(list)
    for number, line in enumerate(raw.decode("utf-8", errors="replace").splitlines(), 1):
        if diagnostic(line):
            diagnostics[normalize(line.strip())].append({"line": number, "text": line})
    return {"name": path.name, "sha256": hashlib.sha256(raw).hexdigest(),
            "lines": len(raw.splitlines()), "diagnostics": dict(diagnostics)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("logs", type=Path, nargs=2)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    logs = [read_log(path) for path in args.logs]
    messages = sorted(set(logs[0]["diagnostics"]) | set(logs[1]["diagnostics"]))
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "log-comparison.json").write_text(json.dumps(logs, indent=2) + "\n")
    report = ["# Supplied runtime log comparison", "",
              "Counts are recorded log occurrences, including suppressed-message notices. "
              "Rate-limited logs cannot establish the underlying call count. "
              "Pointers are grouped; GL codes and invalid receiver values remain distinct.", ""]
    for log in logs:
        report += [f"- `{log['name']}`: {log['lines']} lines, "
                   f"{len(log['diagnostics'])} distinct diagnostics, "
                   f"{sum(map(len, log['diagnostics'].values()))} occurrences; "
                   f"SHA-256 `{log['sha256']}`."]
    common = set(logs[0]["diagnostics"]) & set(logs[1]["diagnostics"])
    report += ["", f"Common diagnostics: **{len(common)}**. "
               f"Only in first: **{len(set(logs[0]['diagnostics']) - common)}**. "
               f"Only in second: **{len(set(logs[1]['diagnostics']) - common)}**.", "",
               "| Diagnostic | First log | Second log |", "|---|---:|---:|"]
    for message in messages:
        counts = [len(log["diagnostics"].get(message, [])) for log in logs]
        message = message.replace("|", "\\|").replace("`", "'")
        report.append(f"| `{message}` | {counts[0]} | {counts[1]} |")
    (args.out / "log-comparison.md").write_text("\n".join(report) + "\n")
    print(f"{len(messages)} distinct diagnostics; {len(common)} common")


if __name__ == "__main__":
    main()
