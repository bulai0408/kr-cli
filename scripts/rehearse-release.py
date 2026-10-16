#!/usr/bin/env python3
"""Install a prepared archive through a disposable local Homebrew tap."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import uuid

from release import ROOT, render_formula


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dist", type=Path, help="Directory produced by release.sh")
    args = parser.parse_args()
    dist = args.dist.resolve()
    sha, archive_name = (dist / "SHA256SUMS").read_text().split()
    archive = dist / archive_name
    version = archive_name.removeprefix("kr-").removesuffix("-aarch64-apple-darwin.tar.gz")
    brew = shutil.which("brew")
    if not brew:
        raise SystemExit("Homebrew is required.")
    tap = "kr46/rehearsal-" + uuid.uuid4().hex[:12]
    formula_name = tap + "/kr"
    # Do not replace an existing installation, including another worker's rehearsal.
    if subprocess.run([brew, "list", "--versions", "kr"], capture_output=True).stdout.strip():
        raise SystemExit("kr is already installed through Homebrew; use a separate machine/user.")
    with tempfile.TemporaryDirectory(prefix="kr46-home-") as temporary:
        home = Path(temporary)
        env = {k: v for k, v in os.environ.items()
               if not k.startswith(("KR_", "HOMEBREW_"))}
        env.update({
            "HOME": str(home), "CODEX_HOME": str(home / ".codex"),
            "XDG_CONFIG_HOME": str(home / ".config"),
            "XDG_STATE_HOME": str(home / ".local/state"),
            "PATH": f"{Path(brew).parent}:/usr/bin:/bin:/usr/sbin:/sbin",
            "HOMEBREW_NO_AUTO_UPDATE": "1", "HOMEBREW_NO_ANALYTICS": "1",
            "HOMEBREW_NO_INSTALL_CLEANUP": "1", "HOMEBREW_NO_ENV_HINTS": "1",
            "HOMEBREW_DEVELOPER": "1",
            "HOMEBREW_CACHE": str(home / "cache"),
            "HOMEBREW_LOGS": str(home / "logs"),
            "HOMEBREW_TEMP": str(home / "tmp"),
        })
        (home / "tmp").mkdir()
        if shutil.which("rustc", path=env["PATH"]):
            raise SystemExit("Rehearsal PATH contains rustc; remove it before this acceptance run.")

        def call(*command, **kwargs):
            return subprocess.run(command, cwd=home, env=env, text=True, check=True, **kwargs)

        tapped = installed = False
        try:
            call(brew, "tap-new", tap, "--no-git")
            tapped = True
            repo = Path(call(brew, "--repository", tap, capture_output=True).stdout.strip())
            template = (ROOT / "packaging/homebrew-tap/Formula/kr.rb").read_text()
            (repo / "Formula/kr.rb").write_text(render_formula(
                template, version, sha, archive.as_uri()))
            call(brew, "install", formula_name)
            installed = True
            prefix = Path(call(brew, "--prefix", formula_name, capture_output=True).stdout.strip())
            binary = str(prefix / "bin/kr")
            result = call(binary, "--version", capture_output=True)
            assert result.stdout.strip() == "kr " + version, result.stdout
            result = call(binary, "skill", capture_output=True)
            assert "build and test Apple projects" in result.stdout
            call(brew, "test", formula_name)
            result = call(binary, "setup", "--json", capture_output=True)
            setup = json.loads(result.stdout)
            assert setup["schema_version"]
            assert (home / ".codex/AGENTS.md").is_file()
            assert (home / ".claude/CLAUDE.md").is_file()
            assert (home / ".agents/skills/kr/SKILL.md").is_file()
            doctor = call(binary, "doctor", "--json", capture_output=True)
            assert json.loads(doctor.stdout)["schema_version"]
            call(binary, "setup", "--remove", "--json", capture_output=True)
            assert not (home / ".agents/skills/kr/SKILL.md").exists()
            assert not (home / ".claude/skills/kr/SKILL.md").exists()
            print("PASS: install, version, skill, brew test, setup, doctor, setup removal; no Rust in PATH.")
        finally:
            if installed:
                call(brew, "uninstall", formula_name)
            if tapped:
                call(brew, "untap", tap)
        assert not (Path(subprocess.check_output([brew, "--cellar"], text=True).strip()) / "kr").exists()
        print("PASS: binary uninstalled, local tap removed; disposable HOME will be deleted.")


if __name__ == "__main__":
    main()
