#!/usr/bin/env bash
# Checks the mim spec in model/ against itself and against the Rust tests.
#   check.sh [REPO_ROOT]
set -euo pipefail
root=${1:-$(cd "$(dirname "$0")/.." && pwd)}

python3 - "$root" <<'PY'
import os, re, sys
root = sys.argv[1]
model = os.path.join(root, "model")

tests = set()
for crate in ("mim", "morloc-deps"):
    for d, _, files in os.walk(os.path.join(root, crate, "src")):
        for fn in files:
            if fn.endswith(".rs"):
                text = open(os.path.join(d, fn), encoding="utf-8", errors="replace").read()
                tests.update(re.findall(r"\bfn\s+([a-z0-9_]+)\s*\(", text))

errors = []
items = {}
prefix_file = {}
for fn in sorted(os.listdir(model)):
    if not fn.endswith(".md") or fn == "README.md":
        continue
    raw = open(os.path.join(model, fn), "rb").read()
    for n, line in enumerate(raw.split(b"\n"), 1):
        if any(b > 0x7E or (b < 0x20 and b != 0x09) for b in line):
            errors.append(f"{fn}:{n}: non-ASCII character")
    item = None
    for n, line in enumerate(raw.decode("utf-8", "replace").split("\n"), 1):
        m = re.match(r"### ([A-Z]+)-(\d+) ", line)
        if m:
            item = f"{m[1]}-{m[2]}"
            if item in items:
                errors.append(f"{fn}:{n}: {item} also defined at {items[item]['where']}")
            owner = prefix_file.setdefault(m[1], fn)
            if owner != fn:
                errors.append(f"{fn}:{n}: prefix {m[1]} belongs to {owner}")
            items[item] = {"intent": None, "code": None, "tests": [], "where": f"{fn}:{n}"}
            continue
        if item is None:
            continue
        d = items[item]
        if line.startswith("Intent:"):
            if not re.fullmatch(r"Intent: (ruled \d{4}-\d\d-\d\d|proposed|open|retired \d{4}-\d\d-\d\d)", line):
                errors.append(f"{fn}:{n}: malformed Intent")
            d["intent"] = line.split()[1]
        elif line.startswith("Code:"):
            if not re.fullmatch(r"Code: (unaudited|conforms \d{4}-\d\d-\d\d|deviates \S.*)", line):
                errors.append(f"{fn}:{n}: malformed Code")
            d["code"] = line
        elif line.startswith("Tests:"):
            d["tests"] = [t.strip() for t in line[6:].split(",") if t.strip()]

for item, d in items.items():
    if d["intent"] is None:
        errors.append(f"{d['where']}: {item} has no Intent")
    if d["code"] is None:
        errors.append(f"{d['where']}: {item} has no Code")
    if d["intent"] == "ruled" and not d["tests"]:
        errors.append(f"{d['where']}: {item} is ruled but cites no test")
    if d["intent"] in ("open", "retired") and d["tests"]:
        errors.append(f"{d['where']}: {item} is {d['intent']} but cites tests")
    for t in d["tests"]:
        if t not in tests:
            errors.append(f"{d['where']}: test {t} does not exist")

for e in errors:
    print(e)
counts = {k: sum(1 for d in items.values() if d["intent"] == k) for k in ("ruled", "proposed", "open", "retired")}
dev = sum(1 for d in items.values() if d["code"] and d["code"].startswith("Code: deviates"))
print(f"{len(items)} items ({', '.join(f'{v} {k}' for k, v in counts.items())}), "
      f"{dev} deviating, {len(errors)} problems")
sys.exit(1 if errors else 0)
PY
