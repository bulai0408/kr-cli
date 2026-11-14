# Resource locks for scripts
Replace the entire old mkdir/owner/trap lock dance with:

    kr lock build --task legacy-build -- bash scripts/build.sh
    kr lock light --task module-test -- swift test --filter MyModuleTests

No project config is required for `kr lock`. One call takes one slot; use the
capacity from `kr doctor --json`, including its source and config overrides.
For simulator scripts use `kr pool lease -- <command>` in a configured project:
it takes a sim slot, leases one pool device, sets KR_SIM_UDID/KR_SIM_NAME, and
shuts it down on return. `kr lock sim -- <command>` takes only a slot, not a
device; never start multiple simulators inside that slot. Build and test in
separate slots. Do not take the old mkdir lock inside either command.

Use `kr pool lease --name Krill-Verify-1 -- <command>` or `--udid <UDID>` to wait
for exactly one project pool device. --name, --udid and explicit --kind conflict;
without them, kind defaults to `default`. A busy target never falls back to another
device. Notices name the device, UDID and holder task; `kr queue` tasks show the target.
Unknown targets exit 2: use `kr pool status`. Waiting for a device still holds a sim
slot, keeping load -> legacy lock -> sim slot -> device lock order (ADR 0023).
With --json, lease notices go to stderr with selection {by: name|udid|kind, value};
sim_leased/sim_released events carry fields.selection. Child stdout stays unchanged.

`kr lock` passes through stdout, stderr and the child's exit code, including 4.
A child killed by signal n returns 128+n. Queue cancellation and kr errors return 1; argument errors
return 2. Queue notices go to stderr; --json does not wrap the child's output.
SIGINT/SIGTERM/SIGHUP are forwarded to the child's group; slots return on exit.
A waiter killed by a signal before it got a slot (often a tool timeout) keeps its
place for 10 minutes: run the same --task from the same worktree again and it goes
back to that place. Its place never holds others back while it is away. When `kr queue`
estimates a wait longer than your tool's timeout, start kr lock in the background and
poll it instead of letting the timeout kill it.
Nested locks re-enter only after checking that an ancestor owns the slot.
Keep [legacy] until the migration checklist is confirmed: install the new kr on
each machine, switch scripts, switch AGENTS.md, then remove [legacy].
`kr doctor` lists non-kr legacy owners observed in the past seven days; its
absence of observations cannot prove all scripts were migrated.
