#!/usr/bin/env python3
"""Scan this repo for personal data: current files, then all published history.

    scripts/scan-personal-data.py            both
    scripts/scan-personal-data.py --tree     tracked and new, non-ignored files
    scripts/scan-personal-data.py --history  blobs and commit metadata on branches,
                                             remotes and tags (not stash or agent refs)

Prints where and which pattern, never the matched value; a path that itself matches
prints as a hash. Exits 1 on any match not covered by scripts/scan-personal-data.allow,
0 otherwise, 2 if it cannot run. A file too large to scan counts as a match.

The owner's names are read from git authors so that this public script never has to
contain them. To also look for names that must not be published (family, employer),
point HX_SCAN_PRIVATE_TERMS at a file outside the repo with one term per line.
"""
import argparse
import hashlib
import os
import pathlib
import re
import subprocess
import sys
from collections import defaultdict

ROOT = pathlib.Path(__file__).resolve().parent.parent
ALLOW_FILE = ROOT / "scripts" / "scan-personal-data.allow"
MAX_BYTES = 20 * 1024 * 1024
HISTORY_REFS = ["--branches", "--remotes", "--tags", "HEAD"]
# Marks authored test data, the convention used across this repo's tests and fixtures.
MADE_UP = re.compile(r"(?i)synthetic|made-up|placeholder|example")
# Words in author identities that are not anybody's name.
NOT_A_NAME = {"code", "github", "noreply", "users", "t3code", "claude", "codex", "devin"}
CAPTURED_PATH = re.compile(r"(^|/)(fixtures|snapshots)/")

EMAIL = re.compile(r"(?<![\w.+-])[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}\b")
EMAIL_OK = re.compile(
    r"(?i)^git@github\.com$|^noreply@(?:github|anthropic|openai)\.com$|@users\.noreply\.github\.com$"
    r"|@example\.(com|org|net)$|\.(test|invalid|example|localhost)$"
)
HOME_PATH = re.compile(r"(?<![\w/.-])(?:/Users|/home|[A-Za-z]:\\+Users)[/\\]+([A-Za-z0-9._-]+)")
HOME_OK = {"you", "me", "user", "username", "name", "private", "synthetic", "example", "runner", "shared"}
PATTERNS = {
    "api-key": re.compile(
        r"\bsk-(?:ant-)?[A-Za-z0-9_-]{20,}|\bgh[pousr]_[A-Za-z0-9]{30,}|\bgithub_pat_[A-Za-z0-9_]{20,}"
        r"|\bAKIA[0-9A-Z]{16}\b|\bAIza[0-9A-Za-z_-]{35}\b|\bxox[abprs]-[A-Za-z0-9-]{10,}|\bya29\.[A-Za-z0-9_-]{20,}"
    ),
    "private-key": re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
    "jwt": re.compile(r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]+"),
    "bearer-token": re.compile(r"(?i)\bbearer\s+[A-Za-z0-9._~+/-]{24,}"),
    "secret-value": re.compile(
        r"""(?i)\b(?:api[_-]?key|secret|token|passw(?:or)?d)["']?\s*[:=]\s*["'][^"'\s$<{]{16,}["']"""
        r"""|(?m:^\s*(?:export\s+)?[A-Z0-9_]*(?:API_?KEY|SECRET|TOKEN|PASSW(?:OR)?D)[A-Z0-9_]*=[^\s"'$<{]{16,})"""
    ),
    "url-credentials": re.compile(r"\b[a-z][a-z0-9+.-]*://[^/\s:@\"']+:[^/\s@\"'{]+@(?!localhost\b|127\.0\.0\.1\b|example\.|[^/\s]*\.(?:test|invalid)\b)"),
    "phone": re.compile(r"(?<![\w+.])\+\d{10,14}\b"),
    "tailnet-host": re.compile(r"\b[a-z0-9-]+\.[a-z0-9-]+\.ts\.net\b"),
    "window-title": re.compile(
        r"(?<=[^\s\"']) - (?:Google Chrome|Chromium|Safari|Arc|Firefox|Microsoft Edge|Brave|Slack|Gmail|Outlook)\b"
        r"|\(Person \d+\)"
    ),
    "private-doc-url": re.compile(
        r"(?i)\b(?:mail\.google\.com/mail/|docs\.google\.com/(?:document|spreadsheets|presentation)/d/"
        r"|[a-z0-9-]+\.slack\.com/archives/|notion\.so/[a-z0-9-]+)"
    ),
}
# A secret is only made up if it says so itself; nearby prose proves nothing.
SECRETS = {"api-key", "private-key", "jwt", "bearer-token", "secret-value", "url-credentials"}
# Prose in a recorded-data file: fixtures hold hashes, x-runs and short authored labels.
CAPTURED_PROSE = re.compile(r"[A-Za-z]{2,}(?:[ \t]+[A-Za-z]{2,}){5,}")
MAIL_HEADER = re.compile(r"(?m)^\W*(?:From|To|Cc|Subject):\s+\S")


def git(*args):
    return subprocess.run(["git", "-C", str(ROOT), *args], check=True, capture_output=True).stdout


def identity_terms():
    log = git("log", *HISTORY_REFS, "--format=%an%n%ae%n%cn%n%ce").decode(errors="replace")
    config = subprocess.run(
        ["git", "-C", str(ROOT), "config", "--get-regexp", r"^user\.(name|email)$"],
        capture_output=True,
        text=True,
    ).stdout
    identities = set(log.splitlines()) | {line.split(" ", 1)[-1] for line in config.splitlines()}
    terms = set()
    for identity in identities:
        for word in re.split(r"[^A-Za-z]+", identity.split("@")[0]):
            if len(word) >= 4 and word.lower() not in NOT_A_NAME:
                terms.add(word.lower())
    return terms


def term_pattern(terms):
    if not terms:
        return None
    parts = [re.escape(t) if len(t) >= 5 else rf"\b{re.escape(t)}\b" for t in sorted(terms, key=len, reverse=True)]
    return re.compile("(?i)" + "|".join(parts))


def private_terms():
    path = os.environ.get("HX_SCAN_PRIVATE_TERMS")
    if not path:
        return set()
    return {line.strip().lower() for line in pathlib.Path(path).read_text().splitlines() if line.strip()}


def made_up(text, match):
    return MADE_UP.search(text, max(0, match.start() - 120), match.end()) is not None


class Scanner:
    def __init__(self):
        self.names = term_pattern(identity_terms())
        self.private = term_pattern(private_terms())

    def count(self, text, path):
        """Pattern name -> number of matches. Only counts leave this function."""
        hits = defaultdict(int)
        if self.names:
            hits["identity-name"] = sum(1 for _ in self.names.finditer(text))
        if self.private:
            hits["private-term"] = sum(1 for _ in self.private.finditer(text))
        hits["email"] = sum(1 for m in EMAIL.finditer(text) if not EMAIL_OK.search(m.group()))
        hits["home-path"] = sum(1 for m in HOME_PATH.finditer(text) if m.group(1).lower() not in HOME_OK)
        for name, pattern in PATTERNS.items():
            hits[name] = sum(
                1
                for m in pattern.finditer(text)
                if not (MADE_UP.search(m.group()) if name in SECRETS else made_up(text, m))
            )
        if CAPTURED_PATH.search(path) and not path.endswith("README.md"):
            hits["captured-prose"] = sum(1 for m in CAPTURED_PROSE.finditer(text) if not made_up(text, m))
            hits["mail-header"] = sum(1 for _ in MAIL_HEADER.finditer(text))
        return {k: v for k, v in hits.items() if v}


def load_allowlist():
    allowed = {}
    for number, line in enumerate(ALLOW_FILE.read_text().splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) != 4 or not fields[3].strip() or not (fields[2] == "*" or fields[2].isdigit()):
            sys.exit(f"scan: {ALLOW_FILE.name}:{number}: need path<TAB>pattern<TAB>max-count-or-*<TAB>reason")
        allowed[(fields[0], fields[1])] = None if fields[2] == "*" else int(fields[2])
    return allowed


class Report:
    def __init__(self, allowed):
        self.allowed = allowed
        self.findings = defaultdict(lambda: {"largest": 0, "versions": 0})
        self.approved = 0

    def add(self, scope, path, hits):
        for pattern, count in hits.items():
            key = (path, pattern)
            if key in self.allowed and (self.allowed[key] is None or count <= self.allowed[key]):
                self.approved += 1
                continue
            finding = self.findings[(scope, path, pattern)]
            finding["largest"] = max(finding["largest"], count)
            finding["versions"] += 1

    def finish(self):
        for (scope, path, pattern), f in sorted(self.findings.items()):
            print(f"{scope}\t{path}\tpattern={pattern}\tmatches={f['largest']}\tversions={f['versions']}")
        print(f"scan: {len(self.findings)} unapproved, {self.approved} approved")
        return 1 if self.findings else 0


def as_text(data):
    """Everything is scanned: UTF-16 and text inside binaries show up once NULs are gone."""
    if data[:2] in (b"\xff\xfe", b"\xfe\xff"):
        return data.decode("utf-16", errors="replace")
    return data.replace(b"\0", b"").decode(errors="replace")


def scan_file(scanner, report, scope, path, data):
    name_hits = scanner.count(path, path)
    # Metadata labels start with "metadata:", so a file can't borrow their approvals.
    shown = f"<path {hashlib.sha256(path.encode()).hexdigest()[:12]}>" if name_hits else path
    shown = f"./{shown}" if shown.startswith("metadata:") else shown
    report.add(scope, shown, {f"filename-{k}": v for k, v in name_hits.items()})
    if len(data) > MAX_BYTES:
        report.add(scope, shown, {"too-large-to-scan": 1})
    else:
        report.add(scope, shown, scanner.count(as_text(data), path))


def scan_tree(scanner, report):
    for raw in git("ls-files", "-z", "-co", "--exclude-standard").split(b"\0"):
        path = raw.decode(errors="replace")
        file = ROOT / path
        if not path or not (file.is_symlink() or file.is_file()):
            continue
        data = os.readlink(file).encode() if file.is_symlink() else file.read_bytes()
        scan_file(scanner, report, "tree", path, data)


def history_blobs():
    paths = {}
    for line in git("rev-list", "--objects", *HISTORY_REFS).decode(errors="replace").splitlines():
        oid, _, path = line.partition(" ")
        if path:
            paths[oid] = path
    process = subprocess.Popen(
        ["git", "-C", str(ROOT), "cat-file", "--batch"], stdin=subprocess.PIPE, stdout=subprocess.PIPE
    )
    # Ask for one object at a time so the pipe cannot fill up in both directions.
    for oid, path in paths.items():
        process.stdin.write(oid.encode() + b"\n")
        process.stdin.flush()
        _, kind, size = process.stdout.readline().split()
        data = process.stdout.read(int(size))
        process.stdout.read(1)
        if kind == b"blob":
            yield path, data
    process.stdin.close()
    process.wait()


def scan_history(scanner, report):
    for path, data in history_blobs():
        scan_file(scanner, report, "history", path, data)
    fields = {"metadata:commit-author": "%an%n%ae%n%cn%n%ce", "metadata:commit-message": "%B"}
    for label, fmt in fields.items():
        for entry in git("log", *HISTORY_REFS, f"--format={fmt}%x00").decode(errors="replace").split("\0"):
            if entry.strip():
                report.add("history", label, scanner.count(entry, label))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--tree", action="store_true")
    parser.add_argument("--history", action="store_true")
    args = parser.parse_args()
    everything = not (args.tree or args.history)
    try:
        scanner = Scanner()
        report = Report(load_allowlist())
        if args.tree or everything:
            scan_tree(scanner, report)
        if args.history or everything:
            scan_history(scanner, report)
    except (OSError, subprocess.CalledProcessError) as error:
        print(f"scan: could not run: {type(error).__name__}", file=sys.stderr)
        return 2
    return report.finish()


if __name__ == "__main__":
    sys.exit(main())
