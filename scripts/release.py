#!/usr/bin/env python3
"""Prepare an arm64 binary and an allowlisted public tap; publishing is opt-in."""

import argparse
import hashlib
import io
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parents[1]
TAP = "bulai0408/homebrew-tap"
TARGET = "aarch64-apple-darwin"
VERSION = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")


def run(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def output(*args, **kwargs):
    return run(*args, stdout=subprocess.PIPE, **kwargs).stdout


def privacy_check(binary, deny_file):
    if not deny_file or not Path(deny_file).is_file():
        raise ValueError("Set KR46_DENY_FILE to a private UTF-8 file with one blocked term per line.")
    terms = [line.strip() for line in Path(deny_file).read_text().splitlines()
             if line.strip() and not line.startswith("#")]
    if not terms:
        raise ValueError("KR46_DENY_FILE must contain blocked terms (names and known team IDs).")
    strings = output("strings", "-a", str(binary))
    raw = Path(binary).read_bytes()
    rules = {
        "local user path": r"/(?:Users|home)/",
        "Cargo registry path": r"\.cargo/registry/",
        "Rustup toolchain path": r"\.rustup/toolchains/",
        "email address": r"(?<![A-Za-z0-9_.+-])[A-Za-z0-9_.+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}(?![A-Za-z0-9_.-])",
        "contextual team ID": r"(?:TeamIdentifier|DEVELOPMENT_TEAM)\s*[=:]\s*[A-Z0-9]{10}\b",
    }
    for label, pattern in rules.items():
        if re.search(pattern, strings):
            raise ValueError(f"Privacy check failed: {label}; no matching value is logged.")
    machine_terms = [("build HOME", str(Path.home())),
                     ("build username", output("id", "-un").strip())]
    for label, term in machine_terms:
        if not term:
            raise ValueError(f"Cannot determine {label}; privacy check refused.")
        if term.casefold() in strings.casefold() or term.encode() in raw:
            raise ValueError(f"Privacy check failed: {label}; no matching value is logged.")
    for term in terms:
        if term.casefold() in strings.casefold() or term.encode() in raw:
            raise ValueError("Privacy check failed: private blocked term; no matching value is logged.")


def command_check(binary, readme):
    help_text = output(str(binary), "--help")
    commands = set(re.findall(r"^\s{2}([a-z][a-z0-9-]*)\s", help_text, re.MULTILINE))
    mentioned = set(re.findall(r"\bkr ([a-z][a-z0-9-]*)\b", readme))
    pending = []
    for command in sorted(mentioned):
        if command in commands:
            continue
        if command == "init":
            pending.append("kr init: pending #47")
        else:
            raise ValueError(f"README command is unavailable in the release binary: kr {command}")
    if "kr skill --reference config" in readme:
        probe = subprocess.run([str(binary), "skill", "--reference", "config"],
                               capture_output=True, text=True)
        if probe.returncode == 2 and "no reference `config`" in probe.stderr:
            pending.append("kr skill --reference config: pending #47")
        elif probe.returncode:
            raise ValueError("Cannot verify kr skill --reference config; fix the binary/runtime error first.")
    return pending


def require_publish_ready(pending):
    if pending:
        raise ValueError("Publish refused: " + "; ".join(pending))


def render_formula(template, version, sha256, url):
    result = template
    for key, value in {"VERSION": version, "SHA256": sha256, "URL": url}.items():
        result = result.replace("@" + key + "@", value)
    if re.search(r"@[A-Z0-9_]+@", result):
        raise ValueError("Unresolved formula template placeholder.")
    return result


def archive_binary(binary, archive):
    with tarfile.open(archive, "w:gz") as tar:
        info = tar.gettarinfo(binary, arcname="kr")
        info.uid = info.gid = info.mtime = 0
        info.uname = info.gname = ""
        info.mode = 0o755
        with Path(binary).open("rb") as source:
            tar.addfile(info, source)


# Public tap commits never carry the local git identity.
TAP_AUTHOR_NAME = "bulai0408"
TAP_AUTHOR_EMAIL = "31983330+bulai0408@users.noreply.github.com"


def publish_commands(dist, version, checkout):
    tag = "v" + version
    return [
        ["gh", "repo", "clone", TAP, str(checkout)],
        ["cp", str(dist / "tap/README.md"), str(checkout / "README.md")],
        ["mkdir", "-p", str(checkout / "Formula")],
        ["cp", str(dist / "tap/Formula/kr.rb"), str(checkout / "Formula/kr.rb")],
        ["git", "-C", str(checkout), "add", "README.md", "Formula/kr.rb"],
        ["git", "-C", str(checkout), "-c", f"user.name={TAP_AUTHOR_NAME}",
         "-c", f"user.email={TAP_AUTHOR_EMAIL}", "commit", "-m", f"kr {version}"],
        ["git", "-C", str(checkout), "push", "origin", "HEAD:main"],
        ["gh", "release", "create", tag, str(dist / f"kr-{version}-{TARGET}.tar.gz"),
         str(dist / "SHA256SUMS"), "--repo", TAP, "--title", f"kr {version}",
         "--notes", f"macOS Apple Silicon binary for kr {version}."],
    ]


def build(ref, version, dist):
    commit = output("git", "-C", str(ROOT), "rev-parse", "--verify", f"{ref}^{{commit}}").strip()
    with tempfile.TemporaryDirectory(prefix="kr-release-") as temporary:
        source = Path(temporary) / "source"
        source.mkdir()
        # Export tracked source only; never copy the private repository to the tap.
        archive = subprocess.check_output(["git", "-C", str(ROOT), "archive", commit])
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(source, filter="data")
        manifest = tomllib.loads((source / "Cargo.toml").read_text())
        if manifest["workspace"]["package"]["version"] != version:
            raise ValueError("Tag/source Cargo version differs; update workspace.package.version before tagging.")
        env = os.environ.copy()
        mappings = [(source, "/src"), (Path.home(), "/build"),
                    (Path(env.get("CARGO_HOME", Path.home() / ".cargo")), "/cargo"),
                    (Path(env.get("RUSTUP_HOME", Path.home() / ".rustup")), "/rustup")]
        flags = []
        for path, replacement in mappings:
            for spelling in {str(path), str(path.resolve())}:
                flags.append(f"--remap-path-prefix={spelling}={replacement}")
        env.pop("RUSTFLAGS", None)
        env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
        env["CARGO_TARGET_DIR"] = str(Path(temporary) / "target")
        env["CARGO_PROFILE_RELEASE_DEBUG"] = "0"
        env["CARGO_PROFILE_RELEASE_STRIP"] = "symbols"
        env["CARGO_INCREMENTAL"] = "0"
        run("kr", "lock", "build", "--task", "kr46-release", "--",
            "cargo", "build", "--locked", "--release", "--target", TARGET, "--bin", "kr",
            cwd=source, env=env)
        shutil.copy2(Path(env["CARGO_TARGET_DIR"]) / TARGET / "release/kr", dist / "kr")
    return commit


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", help="Cargo version, for example 0.1.0 (stable releases only)")
    parser.add_argument("--ref", help="Source ref; defaults to v<VERSION>; non-tag refs require --dry-run")
    parser.add_argument("--output", type=Path, help="New output directory (default target/release-dist/<VERSION>)")
    parser.add_argument("--dry-run", action="store_true", help="Build locally and print, but never run, publish commands")
    parser.add_argument("--publish", action="store_true", help="Publish to the public tap; requires a matching source tag")
    parser.add_argument("--verify-only", type=Path, metavar="BINARY", help="Check an existing binary without building or publishing")
    args = parser.parse_args(argv)
    if not VERSION.fullmatch(args.version):
        raise ValueError("Version must be stable MAJOR.MINOR.PATCH without a v prefix.")
    if args.publish and args.dry_run:
        raise ValueError("Choose either --publish or --dry-run.")
    if args.verify_only and args.publish:
        raise ValueError("--verify-only cannot publish.")
    ref = args.ref or "v" + args.version
    if ref != "v" + args.version and not args.dry_run:
        raise ValueError("A non-tag --ref is allowed only with --dry-run.")
    if not args.verify_only and not args.dry_run:
        output("git", "-C", str(ROOT), "show-ref", "--verify", f"refs/tags/v{args.version}")
    deny_file = os.environ.get("KR46_DENY_FILE")
    if not deny_file or not Path(deny_file).is_file():
        raise ValueError("Set KR46_DENY_FILE before building; see README release instructions.")
    readme = (ROOT / "packaging/homebrew-tap/README.md").read_text()
    if args.verify_only:
        binary = args.verify_only.resolve()
        dist = None
    else:
        dist = (args.output or ROOT / "target/release-dist" / args.version).resolve()
        dist.mkdir(parents=True, exist_ok=False)
        source_ref = f"refs/tags/{ref}" if ref == "v" + args.version else ref
        commit = build(source_ref, args.version, dist)
        binary = dist / "kr"
        (dist / "source-commit.txt").write_text(commit + "\n")
    privacy_check(binary, deny_file)
    if output(str(binary), "--version").strip() != "kr " + args.version:
        raise ValueError("Binary version differs from the requested version.")
    pending = command_check(binary, readme)
    for item in pending:
        print(item)
    if args.publish:
        require_publish_ready(pending)
    if args.verify_only:
        print("PASS: privacy, version and README commands checked (pending items listed above).")
        return
    archive = dist / f"kr-{args.version}-{TARGET}.tar.gz"
    archive_binary(binary, archive)
    sha = hashlib.sha256(archive.read_bytes()).hexdigest()
    (dist / "SHA256SUMS").write_text(f"{sha}  {archive.name}\n")
    tap = dist / "tap"
    (tap / "Formula").mkdir(parents=True)
    (tap / "README.md").write_text(readme)
    formula = render_formula((ROOT / "packaging/homebrew-tap/Formula/kr.rb").read_text(),
                             args.version, sha,
                             f"https://github.com/{TAP}/releases/download/v{args.version}/{archive.name}")
    (tap / "Formula/kr.rb").write_text(formula)
    print(f"PASS: prepared {archive.name}, SHA256SUMS and tap/ (arm64 only).")
    if args.dry_run:
        print("DRY RUN: public operations below were NOT executed; pending #47 is allowed.")
        for command in publish_commands(dist, args.version, Path("<temporary-tap>")):
            print(shlex.join(command))
    elif args.publish:
        with tempfile.TemporaryDirectory(prefix="kr-tap-publish-") as temporary:
            for command in publish_commands(dist, args.version, Path(temporary) / "tap"):
                run(*command)
    else:
        print("Local preparation only. No public operation executed.")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        # Child output is not copied into this message; privacy failures never print terms.
        print(f"ERROR: {error}", file=sys.stderr)
        sys.exit(1)
