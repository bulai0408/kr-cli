# kr

**kr** builds and tests Apple projects and shares machine resources through queues.

1. Check the prerequisites: macOS on Apple Silicon, Xcode with its command-line
   tools selected, and Homebrew. Run `uname -m`, `xcode-select -p`, and
   `brew --version`. If a prerequisite is missing, ask the user before installing
   it or changing the selected Xcode. Intel Macs are not supported by this tap.

2. Install the binary. Rust and access to the source repository are not needed:

   ```sh
   brew install bulai0408/tap/kr
   kr --version
   brew test bulai0408/tap/kr
   ```

3. Ask the user before installing agent discovery entries and rules. Explain the
   scope and wait for an answer. For this project only, run from its root:

   ```sh
   kr setup --project
   ```

   If the user explicitly chooses all projects, run `kr setup` instead. This
   changes the user's agent configuration. Do not run both scopes by default.

4. Read the instructions that ship with this version:

   ```sh
   kr skill
   kr skill --reference config
   ```

5. In the project root, keep an existing `.kr.toml`. If it is absent, run:

   ```sh
   kr init --json
   ```

   Read the output and each `decisions` item. Ask the user about choices that need
   project knowledge, such as schemes, targets, signing, credentials, devices, or
   environments. Do not guess or overwrite an existing configuration. Follow
   `kr init --help` and the configuration reference to apply the answers. Resolve
   decisions before you continue.

6. Verify the machine and project, then report the result to the user:

   ```sh
   kr doctor --json
   kr check --json
   ```

   Read failures and decisions before retrying. Report what passed, what failed,
   what remains unverified, and the next required action. A successful check is
   not evidence that tests ran. Do not claim an unavailable device or service was
   verified.

7. To uninstall, first remove discovery entries in each scope you installed,
   with the user's approval:

   ```sh
   kr setup --project --remove
   ```

   For a global installation use `kr setup --remove` instead. Then remove the
   binary and tap:

   ```sh
   brew uninstall kr
   brew untap bulai0408/tap
   ```

The binary is not signed with a Developer ID or notarized. Prefer Homebrew
installation; Homebrew normally does not apply browser download quarantine to
formula installations. A browser download can trigger Gatekeeper. Verify the
archive checksum against the release, then use System Settings > Privacy &
Security > Open Anyway only if you trust the download and macOS offers that
option. Ask the user to perform this approval. Do not disable Gatekeeper or
remove quarantine automatically.
