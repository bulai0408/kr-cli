# Decision points

kr marks the places its rules do not cover, records the decisions and checks them
later. It does not judge them itself and calls no model (ADR 0011).

## What a decision point has
- `id`: `<run id>-<n>`, the first argument of `kr decide`.
- `kind`: what is to decide (below). `affects`: `conclusion` (the run waits: exit 4) or
  `efficiency` (kr went on with its default; the decision is recorded as pending review).
- `question`, `item`, `test`.
- `options`: each `name` with what choosing it `means`.
- `evidence`: paths and statistics to judge from.
- `default`: kr's suggestion (`option`) and its `reason`.

## Handing a decision back
- `kr decide <id> <option> --reason "..."`. With several decision points in a run, decide
  each; until the last one, kr exits 4 again and lists the rest.
- Or on the next run command: `kr run --decide <id>=<option> --reason "..."` (also
  `kr gate --decide`). It decides; it does not run anything.
- When all are decided, kr prints the run's verdict from the results it has and exits
  0 or 1. A decision is not changed afterwards (exit 2).
- The reason is kept with the decision: name what in the evidence made the choice.

## failure_kind: a failure kr has not seen
A test failed, failed again when run on its own, and kr has not seen this failure of
this test before (same test, same failure signature in the past 90 days). Nothing else
fails the item, so the classification decides its verdict. A failure kr has seen before
fails the item by kr's rules; there is no decision point then.

Options:
- `product`: the code or the test is wrong. The test stays failed; the item fails.
- `flaky`: it fails now and then. The test counts as flaky, as after a passing rerun.
- `environment`: the simulator, the machine or a service caused it. The test is
  unverified, which is not a pass: run it again when the environment is fixed.

Evidence: `signature` and `category` (from the failure message: assertion, timeout,
crash, launch, infrastructure, unknown), `summary`, `xcresult` and `log` (open them),
`load` and `high_load` (the machine load during the item), `history` (in how many
earlier runs the test ran, failed, was flaky).

kr suggests `environment` when the message names the infrastructure or a launch, else
`product`: a failure that repeats on its own rerun is real until shown otherwise.

How to judge:
- Open the xcresult. An assertion about the app's state or data is `product` unless you
  can show that the environment produced that state.
- `flaky` needs a reason why the same code passes at other times (timing, ordering,
  animation) and an earlier run where the test passed.
- `environment` needs a sign outside the app: the runner lost its connection, the
  simulator did not boot, the machine was under high load.

Reconciliation: the next run of the same test on the same build inputs answers the
decision. `product` is supported if the test fails again; `flaky` and `environment` if
it passes. kr writes a `reconciliation` event once per decision.

## additional_scenarios: uncertain change impact

This affects efficiency. kr adds all candidates by default, continues, and records
`via: default`, `review: pending`. It does not wait for `kr decide`.

Inspect `evidence.unknown`, `candidates`, and the 90-day project `history`: pairwise
cofailures count runs where both scenarios had a final result; flaky rate counts
scenario runs with at least one flaky result. Read denominators; null means no sample.
These statistics do not remove scenarios required by the dependency closure.

Use `kr plan --changed <ref> --explain` to inspect file -> module -> consumer -> scenario.
Add missing page edges under `[modules]` and coverage in `scenarios.<name>.modules`.
Declare no-impact glob paths under `[selection].ignore`; kr has no built-in ignores.
Use `--also <scenario>` to add coverage. Recheck the plan after updating mappings.

## Merge resolutions

`kr merge predict --base <ref> <branch>... --json` compares branch pairs. Same-file
changes can need a `merge_resolution` decision even when the text merge is clean.
Read both commits, issue references, overlap and conflicts. Missing issue references
are marked explicitly; inspect the requirements before choosing. Ancestors come first
in the suggested order; pairwise predictions do not guarantee cumulative compatibility.

Use `kr decide <id> combine|ours|theirs|manual|reject --reason "..."`. Here ours and
theirs mean the decision's left and right. This records a resolution; it does not
merge code or prove correctness. Until verified, it exits 1 with verification pending.
After applying and committing a merge containing both sides, run the affected tests,
then `kr merge check --base <before-merge> --decision <id> --verification-run <run-id>`.
Reconciliation requires a clean committed tree, an incremental build and a later
nonzero test run on that tree. Zero tests, missing evidence and an old tree cannot
support a resolution. Compilation or test failure records it as unsupported.
