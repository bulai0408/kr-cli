# kr: build and test Apple projects

This text comes with the kr you run (version {version}). Read it again when kr changes.

## Usual flow
1. `kr doctor`: the machine and the project are ready (Xcode, runtimes, simulator pool).
2. `kr check`: generate, compile with the app's settings, check test filters and data
   isolation. It holds only a `build` slot. Fix what it reports before you go on.
3. `kr run <scenario|suite>...` or `kr gate [name]`: build once, run on pool
   simulators, verify the results. `kr plan` shows what a run would do, without doing it.

Read results with `--json`. Every document has `schema_version` and `decisions`; fields
are only added, never changed.

## Exit codes
- 0 passed; 1 failed, unverified, or kr failed; 2 bad arguments.
- 4 no verdict yet: the run stopped at decision points (only `check`, `run`, `gate`,
  `decide`, `merge predict`). Hand them back, do not run again.
- 3 only `kr plan --exit-code`: there is work to do. 128 + n: stopped by signal n.

## Decision points
For a conclusion decision, the run still runs every item, then exits 4 with
`decisions`: each has an `id`, a question, options, evidence and kr's default with its
reason. Judge from the evidence (`kr skill --reference decisions`), then:

    kr decide <id> <option> --reason "<what in the evidence made the choice>"

kr records who decided what and why, gives the verdict from the results it has, and
later sets the choice against what happened (reconciliation). The owner sees how often
each kind of judgment was right.

For `affects: efficiency`, kr continues with its default and records pending review.
For `additional_scenarios`, read `kr skill --reference decisions` before changing coverage.

## Ask the owner instead of deciding when
- the evidence supports no option better than the others and the item gates a merge;
- you would call a failure `flaky` or `environment` and the same test failed or was
  flaky before (`evidence.history`);
- the failure touches signing, credentials, real devices or production data.

## Hard rules (kr enforces them; do not work around them)
- kr touches only its pool simulators (`<Project>-Verify-<N>`) and the device you name
  with `--device`. Do not run xcodebuild or simctl for a kr project yourself: kr queues
  heavy work for the whole machine.
- 0 tests is not a pass. Unverified is not a pass.
- Credentials never go into code, logs, fixtures or commits.

## More
- `kr plan --changed <ref> --explain` previews required scenarios; `--also` only adds.
  `kr hash <scenario>` explains the inputs of the existing build cache.
- `kr setup --project` installs discovery entries and marked rules for Claude Code and
  Codex in the current directory. `--claude` / `--codex` selects one agent; without
  `--project`, installation is global. Ask the owner before changing global rules.
  `kr setup --project --remove` removes the project entries and marked blocks.
- Read the first failure log before running again. Command reruns preserve `<name>.log`
  and `<name>.again.log`. Skip a high-load whole-item rerun only when the same test
  failed again individually with normal load measured both before and after that
  rerun. High or unknown solo-rerun load keeps the whole-item rerun. Events keep the evidence.
- `kr skill --reference <name>`: {references}.
- `kr <command> --help`: arguments and exit codes of each command.
- Additions without a new kr: `.kr/skill.md` in the project repo and
  `~/.config/kr/skill.md` for you; kr prints them after this text.
