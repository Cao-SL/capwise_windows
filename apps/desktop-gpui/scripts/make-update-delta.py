#!/usr/bin/env python3
"""Build a Capwise update patch (CWDELTA1) from an older app to a newer one.

    make-update-delta.py <old.app> <new.app> <from-version> <to-version> <output>

The installed app rebuilds the new bundle from its own files plus this patch
(src/update_delta.rs) and checks every file against the hashes recorded here,
so the result is byte-for-byte the signed, notarized bundle. Needs the zstd
CLI (brew install zstd).
"""

import hashlib
import json
import os
import stat
import struct
import subprocess
import sys
import tempfile

MAGIC = b"CWDELTA1"
LEVEL = "-19"
LONG = "--long=27"


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def zstd(*args):
    subprocess.run(["zstd", "-q", "-f", "--no-progress", *args], check=True)


def walk(root):
    dirs, files, links = [], [], []
    for current, subdirs, names in os.walk(root, followlinks=False):
        for name in sorted(subdirs + names):
            path = os.path.join(current, name)
            relative = os.path.relpath(path, root)
            info = os.lstat(path)
            if stat.S_ISLNK(info.st_mode):
                links.append((relative, os.readlink(path)))
            elif stat.S_ISDIR(info.st_mode):
                dirs.append((relative, stat.S_IMODE(info.st_mode)))
            elif stat.S_ISREG(info.st_mode):
                files.append((relative, stat.S_IMODE(info.st_mode)))
            else:
                raise SystemExit(f"unsupported file type: {relative}")
    return sorted(dirs), sorted(files), sorted(links)


def main():
    if len(sys.argv) != 6:
        raise SystemExit(__doc__)
    old, new, version_from, version_to, output = sys.argv[1:]
    dirs, files, links = walk(new)
    entries = [{"kind": "dir", "path": path, "mode": mode} for path, mode in dirs]
    payload = []
    offset = 0
    stats = {"copy": 0, "patch": 0, "data": 0}

    with tempfile.TemporaryDirectory() as work:
        for relative, mode in files:
            target = os.path.join(new, relative)
            source = os.path.join(old, relative)
            digest = sha256(target)
            entry = {"kind": "file", "path": relative, "mode": mode, "sha256": digest}
            source_digest = None
            if os.path.isfile(source) and not os.path.islink(source):
                source_digest = sha256(source)
            if source_digest == digest:
                entry["source"] = "copy"
                stats["copy"] += 1
                entries.append(entry)
                continue

            chosen = None
            if source_digest is not None:
                patch = os.path.join(work, "patch.zst")
                zstd(LEVEL, LONG, f"--patch-from={source}", target, "-o", patch)
                check = os.path.join(work, "check")
                zstd("-d", LONG, f"--patch-from={source}", patch, "-o", check)
                if sha256(check) != digest:
                    raise SystemExit(f"patch for {relative} does not round-trip")
                chosen = patch
                entry["source"] = "patch"
                entry["source_sha256"] = source_digest
            if chosen is None or os.path.getsize(chosen) > os.path.getsize(target) // 4:
                full = os.path.join(work, "full.zst")
                zstd(LEVEL, LONG, target, "-o", full)
                if chosen is None or os.path.getsize(full) < os.path.getsize(chosen):
                    chosen = full
                    entry["source"] = "data"
                    entry.pop("source_sha256", None)
            with open(chosen, "rb") as handle:
                blob = handle.read()
            entry["offset"] = offset
            entry["length"] = len(blob)
            offset += len(blob)
            payload.append(blob)
            stats[entry["source"]] += 1
            entries.append(entry)

    entries += [{"kind": "symlink", "path": path, "target": target} for path, target in links]
    manifest = json.dumps(
        {"from": version_from, "to": version_to, "entries": entries},
        separators=(",", ":"),
    ).encode()
    with open(output, "wb") as out:
        out.write(MAGIC)
        out.write(struct.pack("<Q", len(manifest)))
        out.write(manifest)
        for blob in payload:
            out.write(blob)
    print(
        f"{os.path.basename(output)}: {os.path.getsize(output) / 1e6:.1f} MB "
        f"({stats['copy']} unchanged, {stats['patch']} patched, {stats['data']} new)"
    )


if __name__ == "__main__":
    main()
