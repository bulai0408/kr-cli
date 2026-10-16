# Configuration

Run `kr init` in the project root to create a minimal `.kr.toml`. It reads Xcode
projects/workspaces, XcodeGen `project.yml`, or `Package.swift`, and validates the
result. It does not build, create simulators, or overwrite a file (exit 2).
Simulator runtimes must be installed and meet the deployment target. Native macOS
UI tests use a `smoke` test suite with `destination = "macos"`; SwiftPM uses a
`swift test` command suite. Neither needs a simulator pool.
Review `decisions` from `kr init --json`; edit `.kr.toml` to resolve them.
These are configuration suggestions, not saved run decisions for `kr decide`.
Then run `kr doctor`, `kr pool sync` if needed, and `kr check`.

Project settings belong in `.kr.toml`. Discovery searches upward to the first
`.git` directory/file. Machine settings belong in `~/.config/kr/config.toml`
(`KR_CONFIG` overrides that path; otherwise `XDG_CONFIG_HOME` is respected).
Machine precedence: file < environment < command overrides. For example,
`KR_RESOURCES__BUILD=1` sets `resources.build`. Do not use a separate state
directory or capacity overrides to bypass other agents' queues.
`kr config --json` shows the merged values and sources; invalid TOML and semantic
references report the file, line, key and correction. Unknown fields are errors.

## Project fields

`<name>` and `<kind>` are keys you choose, not literal table names.
Optional fields may be omitted. A default of "unset" passes no override to Xcode.

| Key | Meaning and default | Example |
| --- | --- | --- |
| `project` | Required project identity and build settings. | `[project]` |
| `modules` | Page/module relationships; default empty map. | `[modules.Detail]` |
| `selection` | Changed-file selection policy; default empty. | `[selection]` |
| `simulators` | Project-owned pool; default empty (needed by simulator tests). | `[simulators]` |
| `suites` | Named build/test/command steps; default empty map. | `[suites.unit]` |
| `scenarios` | Named UI test groups; default empty map. | `[scenarios.smoke]` |
| `gates` | Named sets of suites/scenarios; default empty map. | `[gates.default]` |
| `isolation` | Allowed synthetic test identity and entitlements; default empty. | `[isolation]` |
| `signing` | Real-device signing checks; default empty. | `[signing]` |
| `project.name` | Required pool namespace; use a project name. | `"Atlas"` |
| `project.xcodeproj` | Project path relative to root; unset. Mutually exclusive with workspace. | `"Atlas.xcodeproj"` |
| `project.workspace` | Workspace path relative to root; unset. | `"Atlas.xcworkspace"` |
| `project.generate` | Shell command before builds; unset. | `"xcodegen generate --quiet"` |
| `project.scheme` | App/UI test scheme; unset, required by scenarios. | `"Atlas"` |
| `project.configuration` | Xcode configuration; unset (Xcode chooses). | `"Debug"` |
| `project.build_settings` | Additional KEY=VALUE settings on project xcodebuild calls; empty map. | `{ APP_TEST_ID = "dev.example.atlas.e2e" }` |
| `project.xcode` | Required installed Xcode version prefix; unset (selected Xcode). | `"27.1"` |
| `project.platforms` | Build platforms; empty list. Build suites need a platform here or locally. Values: iOS, tvOS, macOS, watchOS, visionOS. | `["iOS", "macOS"]` |
| `project.compilation_cache` | Shared Xcode compilation cache across project worktrees; false. | `true` |
| `simulators.runtime` | Installed runtime name or identifier for all kinds; unset. Required here or per kind. | `"iOS 27.0"` |
| `simulators.devices` | Named device kinds; empty map. `default` produces Atlas-Verify-1; other kinds produce Atlas-Verify-iPad-1. | `[simulators.devices.default]` |
| `simulators.devices.<kind>.device_type` | Required installed device type name or identifier compatible with runtime. | `"iPhone 17 Pro"` |
| `simulators.devices.<kind>.count` | Number of pool devices, at least 1; default 1. Does not override machine sim capacity. | `3` |
| `simulators.devices.<kind>.runtime` | Runtime override for this kind; unset (inherits simulators.runtime). | `"iOS 26.5"` |
| `suites.<name>.type` | Required: build, test, or command. | `"test"` |
| `suites.<name>.dir` | Working directory relative to root; unset (root). Build/test with dir use their own project/package instead of project.xcodeproj/workspace. | `"Packages/Core"` |
| `suites.<name>.scheme` | Required for build/test; not accepted for command. | `"Atlas"` |
| `suites.<name>.platforms` | Build only: platform list; empty inherits project.platforms. | `["macOS"]` |
| `suites.<name>.only` | Test only: -only-testing filters; empty means all scheme tests. | `["AtlasTests"]` |
| `suites.<name>.skip` | Test only: -skip-testing filters; empty. | `["AtlasTests/SlowTests"]` |
| `suites.<name>.destination` | Test only: simulator (default) or macos. | `"macos"` |
| `suites.<name>.device` | Test only: simulator kind; unset means default. | `"iPad"` |
| `suites.<name>.run` | Command only: required shell command, executed with sh -c. SwiftPM may use swift test. | `"swift test"` |
| `suites.<name>.timeout` | Positive duration; unset means 20m during run/gate. | `"15m"` |
| `scenarios.<name>.only` | Required nonempty -only-testing filters: target, target/class, target/class/method. | `["AtlasUITests/LaunchTests"]` |
| `scenarios.<name>.skip` | -skip-testing filters; empty. | `["AtlasUITests/LaunchTests/testSlow"]` |
| `scenarios.<name>.device` | Simulator kind; unset means default. | `"iPad"` |
| `scenarios.<name>.timeout` | Positive hard timeout; unset means 20m. | `"1h30m"` |
| `scenarios.<name>.external` | Depends on a real external service; false. Never reuse test results. | `true` |
| `scenarios.<name>.modules` | Covered module/page names for changed-file selection; empty. | `["Detail"]` |
| `scenarios.<name>.prepare` | Dependency steps before tests, with cleanup after; empty list (ADR 0020). | `[[scenarios.smoke.prepare]]` |
| `scenarios.<name>.prepare[].run` | Required nonempty shell command. Background servers must remain foreground. | `"python3 -m http.server 8080"` |
| `scenarios.<name>.prepare[].dir` | Existing directory relative to root, without ..; unset means root. | `"."` |
| `scenarios.<name>.prepare[].timeout` | Positive setup/readiness timeout; unset means 30s. | `"45s"` |
| `scenarios.<name>.prepare[].background` | Start a service; false (one-shot command). | `true` |
| `scenarios.<name>.prepare[].ready` | Command retried until exit 0; required for background, forbidden otherwise. Unset. | `"curl --fail http://127.0.0.1:8080/"` |
| `scenarios.<name>.prepare[].cleanup` | Optional nonempty shell cleanup command; unset. Owned process groups are stopped automatically. | `"rm -f build/test-ready"` |
| `scenarios.<name>.prepare[].shared` | Nonempty instance name shared by dependent scenarios in one run; unset means per scenario. Definitions with one name must match. | `"http-fixture"` |
| `modules.<name>.paths` | Source files/directories relative to root, without ..; empty. | `["App/Detail"]` |
| `modules.<name>.depends_on` | Module/page dependencies; empty. | `["Core"]` |
| `selection.ignore` | Project-declared globs excluded from changed-file selection; empty, no built-in ignores. | `["docs/**", "**/*.md"]` |
| `gates.<name>.suites` | Existing suite names; empty. | `["unit", "lint"]` |
| `gates.<name>.scenarios` | Existing scenario names or "*" for all; empty. | `["smoke"]` |
| `isolation.bundle_ids` | Allowed synthetic App bundle IDs; empty disables this check. Never copy production IDs here as a shortcut. | `["dev.example.atlas.e2e"]` |
| `isolation.entitlements` | Allowed string values per entitlement; empty. Device signing with iCloud defaults to Development. | `{ "com.apple.developer.icloud-container-environment" = ["Development"] }` |
| `signing.team` | Expected Apple team for real-device builds; unset. Device checks require an explicit matching team. | `"ABCDE12345"` |
| `signing.register_devices` | Allow xcodebuild to register a device with the team; false. This changes the developer account. | `false` |

Preparation failures are unverified, never product failures. Commands receive
KR_RUN_ID, KR_PROJECT, KR_SCENARIO, KR_DEVICE_UDID and KR_RUN_DIR. Shared steps have
empty scenario/UDID. Use instance-specific ports and files; a fixed port in this
example must be changed before parallel runs.

## Complete project example

This example shows every field and all suite types. Adapt paths, filters, installed
runtime/device type, Xcode version, synthetic build settings, and team before use.
Use workspace instead of xcodeproj when needed; never set both.

```toml
[project]
name = "Atlas" # Pool namespace.
xcodeproj = "Atlas.xcodeproj"
# workspace = "Atlas.xcworkspace" # Alternative to xcodeproj.
generate = "xcodegen generate --quiet"
scheme = "Atlas"
configuration = "Debug"
build_settings = { APP_TEST_ID = "dev.example.atlas.e2e" }
xcode = "27.1" # Installed version prefix, not the deployment target.
platforms = ["iOS", "macOS"]
compilation_cache = false

[simulators]
runtime = "iOS 27.0" # Choose an installed runtime.
[simulators.devices.default]
device_type = "iPhone 17 Pro"
count = 3 # Atlas-Verify-1 through Atlas-Verify-3.
runtime = "iOS 27.0" # Optional per-kind override.
[simulators.devices.iPad]
device_type = "iPad Pro 13-inch (M5)"
count = 1 # Atlas-Verify-iPad-1.

[suites.build]
type = "build"
dir = "Packages/Core" # A package builds from this directory.
scheme = "Core-Package"
platforms = ["macOS"]
timeout = "15m"
[suites.unit]
type = "test"
dir = "Packages/Core"
scheme = "Core-Package"
only = ["CoreTests"]
skip = ["CoreTests/SlowTests"]
destination = "macos" # simulator uses the device kind below.
device = "default"
timeout = "10m"
[suites.lint]
type = "command"
dir = "."
run = "swiftlint --strict"
timeout = "90s"

[scenarios.smoke]
only = ["AtlasUITests/LaunchTests"]
skip = ["AtlasUITests/LaunchTests/testSlow"]
device = "default"
timeout = "20m"
external = false
modules = ["Detail"]
[[scenarios.smoke.prepare]]
run = "python3 -m http.server 8080"
dir = "."
timeout = "30s"
background = true
ready = "curl --fail http://127.0.0.1:8080/"
cleanup = "rm -f build/test-ready"
shared = "http-fixture"

[modules.Detail]
paths = ["App/Detail"]
depends_on = ["Core"]
[modules.Core]
paths = ["Packages/Core/Sources"]
depends_on = []
[selection]
ignore = ["docs/**", "**/*.md"]

[gates.default]
suites = ["build", "unit", "lint"]
scenarios = ["smoke"] # "*" selects every registered scenario.
[isolation]
bundle_ids = ["dev.example.atlas.e2e"]
[isolation.entitlements]
"com.apple.developer.icloud-container-environment" = ["Development"]
[signing]
team = "ABCDE12345" # Replace with the actual expected team for device tests.
register_devices = false
```

## Machine fields

These keys are forbidden in `.kr.toml`. Keep all agents on the same state directory.
Do not change an existing legacy bridge until the owner confirms all callers migrated.

| Key | Meaning and default | Example |
| --- | --- | --- |
| `state_dir` | State/events/queues directory; XDG_STATE_HOME/kr, else ~/.local/state/kr. KR_STATE_DIR overrides as an environment setting. | `"/var/tmp/kr-state"` |
| `resources` | Optional manual capacities; default derived from machine profile. | `[resources]` |
| `resources.build` | Heavy build slots; unset means derived, minimum effective 1. | `1` |
| `resources.sim` | Concurrent simulator slots; unset means derived, minimum effective 1. | `2` |
| `resources.light` | Small module checks, no app/simulator; unset means derived, minimum effective 1. | `2` |
| `machines` | Per-LocalHostName overrides; empty map. | `[machines.WorkMac.resources]` |
| `machines.<host>.resources` | Overrides matching host; unset fields inherit resources then derived capacity. | `[machines.WorkMac.resources]` |
| `machines.<host>.resources.build` | Build slots on that host; unset inherits. | `1` |
| `machines.<host>.resources.sim` | Simulator slots on that host; unset inherits. | `2` |
| `machines.<host>.resources.light` | Light slots on that host; unset inherits. | `2` |
| `load` | Load/memory waiting policy; defaults below. | `[load]` |
| `load.block` | Also wait for high 1-minute load; false. Critical memory pressure always waits. | `false` |
| `load.per_core` | High-load threshold times logical cores; 4.0. | `4.0` |
| `load.max_wait` | Maximum wait before continuing; 20m. | `"20m"` |
| `legacy` | Optional old machine lock bridge; off until both paths are set. | `[legacy]` |
| `legacy.machine_lock` | Existing shared mkdir lock path; unset. | `"/tmp/native-build.lock"` |
| `legacy.queue` | Existing shared queue path; unset. | `"/tmp/native-build.queue"` |
| `legacy.poll` | Bridge retry interval; unset means 30s. | `"30s"` |
| `remotes` | Declared SSH hosts only; empty map. SSH owns credentials and host verification. | `[remotes.builder]` |
| `remotes.<name>.ssh` | Required SSH host or ~/.ssh/config alias. | `"build-mac"` |
| `remotes.<name>.dir` | Project clone directory on host; unset means ~/kr-remote. | `"~/kr-remote"` |
| `remotes.<name>.kr` | Remote kr executable; unset means kr from SSH PATH. | `"kr"` |

## Complete machine example

This is a separate file, never append it to the project example.
The state directory and capacities are illustrative, not recommended overrides.

```toml
state_dir = "/var/tmp/kr-state" # All projects/agents must use the same directory.
[resources]
build = 1
sim = 2
light = 2
[machines.WorkMac.resources] # scutil --get LocalHostName
build = 1
sim = 2
light = 2
[load]
block = false
per_core = 4.0
max_wait = "20m"
[legacy] # Only when the machine already uses this protocol.
machine_lock = "/tmp/native-build.lock"
queue = "/tmp/native-build.queue"
poll = "30s"
[remotes.builder] # Run: kr run --remote builder smoke
ssh = "build-mac"
dir = "~/kr-remote"
kr = "kr"
```
