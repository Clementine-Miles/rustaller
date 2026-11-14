#!/usr/bin/env python3
"""Create a deterministic source/artifact archive, excluding caches and secrets."""
import hashlib
import subprocess
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    subprocess.run(["python3", str(ROOT / "build/build.py")], check=True)
    files = [ROOT / "README.md", ROOT / ".gitignore", ROOT / "LICENSE"]
    for folder in ("src", "build", "tests", "docs", "upstream", ".github"):
        files.extend(p for p in (ROOT / folder).rglob("*")
                     if p.is_file() and not p.is_symlink()
                     and "__pycache__" not in p.parts)
    files.extend([ROOT / "dist/dmitbox-local.sh", ROOT / "dist/SHA256SUMS"])
    archive = ROOT / "dist/dmitbox-local-security-refactor.zip"
    contents = []
    for path in sorted(set(files)):
        if path.is_symlink():
            raise SystemExit(f"Unsafe archive symlink: {path}")
        name = "dmitbox-local/" + path.relative_to(ROOT).as_posix()
        mode = 0o100700 if path.stat().st_mode & 0o100 else 0o100600
        contents.append((name, path.read_bytes(), mode))
    manifest = "".join(f"{hashlib.sha256(data).hexdigest()}  {name.removeprefix('dmitbox-local/')}\n"
                       for name, data, _mode in contents).encode()
    contents.append(("dmitbox-local/SHA256SUMS", manifest, 0o100600))
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED,
                         compresslevel=9) as output:
        for name, data, mode in contents:
            info = zipfile.ZipInfo(name, date_time=(2026, 11, 13, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = (mode << 16)
            output.writestr(info, data)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(".zip.sha256").write_text(f"{digest}  {archive.name}\n")
    print(f"{archive} SHA256 {digest}")


if __name__ == "__main__":
    main()
