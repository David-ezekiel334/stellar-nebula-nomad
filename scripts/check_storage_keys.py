#!/usr/bin/env python3
"""Storage-key collision validator (Issue #609).

All Soroban modules in this crate share one contract storage namespace. A
`#[contracttype]` enum variant is encoded as `Vec[Symbol(<variant name>), ..payload]`,
so two key enums that declare a variant with the same name *and* the same
payload types produce byte-identical keys and silently overwrite each other.

This script scans `src/` for storage-key enums (any `#[contracttype]` enum whose
name ends in `Key`/`Keys`, plus enums used as `storage().<tier>().get/set/has/
remove(&Enum::Variant ...)`), then fails if:

  * two key enums define the same variant name with the same payload signature,
  * a variant name is longer than 32 characters (Soroban `Symbol` limit).

With `--strict` it additionally fails on *bare generic* names (`Admin`, `Config`,
`Counter`, ...) that are not module-prefixed. See docs/STORAGE_KEY_CONVENTIONS.md.

Usage: python3 scripts/check_storage_keys.py [--strict] [--src DIR]
"""
import argparse
import collections
import glob
import os
import re
import sys

GENERIC = {
    "Admin", "Config", "Counter", "State", "Paused", "Version", "Metrics",
    "Registry", "Session", "Leaderboard", "Initialized", "Owner", "Total",
}
SYMBOL_MAX = 32


def parse_enums(src):
    out = []
    for path in sorted(glob.glob(os.path.join(src, "**", "*.rs"), recursive=True)):
        text = open(path, encoding="utf-8").read()
        for m in re.finditer(r"#\[contracttype\][^{;]*?pub\s+enum\s+(\w+)\s*\{", text):
            i = m.end()
            depth, j = 1, i
            while depth:
                c = text[j]
                depth += (c == "{") - (c == "}")
                j += 1
            body = re.sub(r"//[^\n]*", "", text[i : j - 1])
            variants, depth, cur = [], 0, ""
            for c in body:
                depth += (c in "(<") - (c in ")>")
                if c == "," and depth == 0:
                    variants.append(cur)
                    cur = ""
                else:
                    cur += c
            variants.append(cur)
            for v in variants:
                v = re.sub(r"#\[[^\]]*\]", "", v).strip()
                if not v:
                    continue
                name = re.match(r"\w+", v).group(0)
                sig = re.sub(r"\s+", "", v[len(name):])
                out.append((path, m.group(1), name, sig))
    return out


def storage_used_enums(src):
    used = set()
    pat = re.compile(
        r"\.(get|set|has|remove|extend_ttl|update|try_update)(::<[^>]*>)?\(\s*&?\s*&?\s*(\w+)::\w+"
    )
    for path in glob.glob(os.path.join(src, "**", "*.rs"), recursive=True):
        for m in pat.finditer(open(path, encoding="utf-8").read()):
            used.add((path, m.group(3)))
    return used


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--src", default=os.path.join(os.path.dirname(__file__), "..", "src"))
    ap.add_argument("--strict", action="store_true", help="also reject bare generic names")
    args = ap.parse_args()

    variants = parse_enums(args.src)
    used = storage_used_enums(args.src)
    keys = [v for v in variants if (v[0], v[1]) in used or re.search(r"Keys?$", v[1])]

    groups = collections.defaultdict(list)
    for path, enum, name, sig in keys:
        groups[(name, sig)].append((os.path.relpath(path, args.src), enum))

    errors = []
    for (name, sig), where in sorted(groups.items()):
        if len(where) > 1:
            errors.append(f"collision: `{name}{sig}` defined in " +
                          ", ".join(f"{p}::{e}" for p, e in where))
    for path, enum, name, _ in keys:
        if len(name) > SYMBOL_MAX:
            errors.append(f"too long ({len(name)} > {SYMBOL_MAX}): {path}::{enum}::{name}")
        if args.strict and name in GENERIC:
            errors.append(f"bare generic name: {os.path.relpath(path, args.src)}::{enum}::{name}")

    n_enums = len({(p, e) for p, e, _, _ in keys})
    if errors:
        print("\n".join(errors))
        print(f"\nFAILED: {len(errors)} problem(s) across {n_enums} key enums")
        return 1
    print(f"OK: {len(keys)} key variants in {n_enums} key enums, no collisions")
    return 0


if __name__ == "__main__":
    sys.exit(main())
