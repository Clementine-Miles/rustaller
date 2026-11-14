#!/usr/bin/env python3
"""Copy one local test report into the documentation; development use only."""
import hashlib
import json
import shutil
import sys
from pathlib import Path


def main() -> None:
    root = Path(__file__).resolve().parents[1]
    if len(sys.argv) != 2:
        raise SystemExit("Usage: python3 tests/publish-evidence.py artifacts/REPORT/results.json")
    report_file = Path(sys.argv[1]).resolve(strict=True)
    report = json.loads(report_file.read_text())
    if report.get("schema") != 1 or not isinstance(report.get("results"), list):
        raise SystemExit("Unsupported evidence schema")
    # Only reports produced inside this checkout's private artifact tree may be
    # published. Never follow arbitrary detail paths from a supplied JSON file.
    report_file.relative_to((root / "artifacts").resolve())
    destination = root / "docs" / "evidence"
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(mode=0o700)
    rows = []
    normalized = dict(report)
    normalized["source_root"] = "."
    normalized["results"] = []
    for result in report["results"]:
        item = dict(result)
        detail = str(item.get("detail", ""))
        candidate = Path(detail)
        if candidate.is_absolute() and candidate.parent.resolve() == report_file.parent:
            if not candidate.is_file() or candidate.is_symlink():
                raise SystemExit(f"Missing or unsafe test log: {candidate}")
            shutil.copyfile(candidate, destination / candidate.name)
            item["detail"] = candidate.name
        rows.append(f"| `{item['name']}` | **{item['status']}** | {item['exit_code']} |")
        normalized["results"].append(item)
    metadata_names = ["environment.txt", "source-sha256.txt"]
    if (report_file.parent / "source-sha256-end.txt").exists():
        metadata_names.append("source-sha256-end.txt")
    for name in metadata_names:
        source = report_file.parent / name
        if not source.is_file() or source.is_symlink():
            raise SystemExit(f"Missing or unsafe evidence metadata: {source}")
        text = source.read_text().replace(str(root) + "/", "")
        (destination / name).write_text(text)
    (destination / "results.json").write_text(json.dumps(normalized, indent=2) + "\n")
    checksums = []
    for path in sorted(destination.iterdir()):
        checksums.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n")
    (destination / "SHA256SUMS").write_text("".join(checksums))
    content = f"""# Test evidence

Recorded at `{report['generated_at']}`. Source checksums and environment/tool versions accompany the [machine-readable report](evidence/results.json). Logs are copied unchanged; their checksums are in `docs/evidence/SHA256SUMS`.

| Check | Result | Exit code |
| --- | --- | --- |
{chr(10).join(rows)}

Reported failures: **{report['failures']}**. Reported skips: **{report['skips']}**. Strict integration was **{'enabled' if report['strict_integration'] else 'disabled'}**.

Reproduce with `python3 build/build.py` and `bash tests/run.sh --strict-integration`, then publish the chosen report using `python3 tests/publish-evidence.py artifacts/REPORT/results.json`. Source hashes identify the tested inputs; rerun after any source change.

A PASS applies only to the assertions in that check. A SKIP is missing evidence. Namespace facilities and mocked service behavior do not prove boot recovery or external accessibility. See [test levels](TESTING.md) and [limitations](LIMITATIONS.md) before interpreting these results as release acceptance.
"""
    (root / "docs" / "TEST-EVIDENCE.md").write_text(content)
    print(root / "docs" / "TEST-EVIDENCE.md")


if __name__ == "__main__":
    main()
