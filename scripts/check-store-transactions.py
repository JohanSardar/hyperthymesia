#!/usr/bin/env python3
"""Guard the single BEGIN IMMEDIATE entry point, including fixtures and tests."""
from pathlib import Path
import re
import sys

allowed = Path("crates/hx-store/src/sqlite/mod.rs")
violations = []
for path in Path("crates").rglob("*.rs"):
    for number, line in enumerate(path.read_text().splitlines(), 1):
        if re.search(r"\.\s*begin(?:_with)?\s*\(", line):
            if path != allowed or line.strip() != 'self.pool.begin_with("BEGIN IMMEDIATE").await?,':
                violations.append(f"{path}:{number}: use Store::begin_write()")
if violations:
    sys.exit("\n".join(violations))
