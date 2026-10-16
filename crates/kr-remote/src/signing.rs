//! Whether a machine can sign with a developer certificate now, decided by signing a
//! temporary file once there (ADR 0014). kr does not unlock or change keychains and
//! never reads or stores credentials; certificate names (which hold an account name or
//! an e-mail address) do not leave the machine: only an identity number, the first
//! characters of its SHA-1 and the team ID do.

use kr_exec::Command;

use crate::Problem;

/// Signs a temporary file with each Apple Development identity and prints one line
/// per identity, then `identities=<N>`. The file is deleted when the script ends.
pub const SCRIPT: &str = r#"f=$(mktemp "${TMPDIR:-/tmp}/kr-trial-sign.XXXXXX") || exit 1
trap 'rm -f "$f"' EXIT
echo kr > "$f"
n=0
for id in $(security find-identity -v -p codesigning 2>/dev/null | awk '/"Apple Development: /{print $2}'); do
  n=$((n+1))
  short=$(printf %s "$id" | cut -c1-8)
  if out=$(codesign --force --sign "$id" "$f" 2>&1); then
    echo "identity $n $short signed team=$(codesign -dv "$f" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
  else
    echo "identity $n $short failed $(printf %s "${out##*: }" | tail -n 1)"
  fi
done
echo "identities=$n""#;

/// The trial sign on this Mac.
pub fn local_command() -> Command {
    Command::new("sh").args(["-c", SCRIPT])
}

/// How one identity did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// "#1 (1A2B3C4D)".
    pub identity: String,
    /// The team it signed for, or why codesign could not sign.
    pub result: Result<String, String>,
}

/// What [`SCRIPT`] printed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrialSign {
    pub attempts: Vec<Attempt>,
    /// Whether the script got to its last line.
    pub finished: bool,
}

impl TrialSign {
    pub fn parse(stdout: &str) -> Self {
        let mut trial = Self::default();
        for line in stdout.lines().map(|l| l.trim_end_matches('\r')) {
            if line.starts_with("identities=") {
                trial.finished = true;
                continue;
            }
            let Some(rest) = line.strip_prefix("identity ") else {
                continue;
            };
            let mut parts = rest.splitn(4, ' ');
            let (Some(n), Some(short), Some(what)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let detail = parts.next().unwrap_or_default().trim().to_owned();
            let result = match what {
                "signed" => Ok(detail.strip_prefix("team=").unwrap_or(&detail).to_owned()),
                _ if detail.is_empty() => Err("codesign gave no reason".to_owned()),
                _ => Err(detail),
            };
            trial.attempts.push(Attempt {
                identity: format!("#{n} ({short})"),
                result,
            });
        }
        trial
    }

    /// Why `machine` ("work", "this Mac") cannot sign `needed_by` (e.g. "`launch`
    /// (platform=iOS,id=…)"), given `[signing].team`. `failure`: the script's stderr
    /// when it did not finish.
    pub fn problems(
        &self,
        machine: &str,
        needed_by: &str,
        team: Option<&str>,
        failure: &str,
    ) -> Vec<Problem> {
        let signed: Vec<&str> = self
            .attempts
            .iter()
            .filter_map(|a| a.result.as_deref().ok())
            .collect();
        if signed.iter().any(|t| team.is_none_or(|team| *t == team)) {
            return Vec::new();
        }
        let of_team = team.map(|t| format!(" of team {t}")).unwrap_or_default();
        let need = format!("{needed_by} needs a developer certificate");
        if !self.finished {
            let why = failure
                .lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty())
                .unwrap_or("it printed nothing");
            return vec![Problem::new(
                "signing_unavailable",
                format!("{need}; the trial sign on {machine} did not finish: {why}"),
                format!(
                    "check that `security find-identity -v -p codesigning` and `codesign` work on {machine}, then run again"
                ),
            )];
        }
        if self.attempts.is_empty() {
            return vec![Problem::new(
                "signing_no_identity",
                format!("{need}; {machine} has no valid Apple Development signing identity"),
                format!(
                    "on {machine}: add an Apple Development certificate{of_team} (Xcode > Settings > Accounts > Manage Certificates), then run again"
                ),
            )];
        }
        if !signed.is_empty() {
            let mut teams = signed.clone();
            teams.sort_unstable();
            teams.dedup();
            return vec![Problem::new(
                "signing_wrong_team",
                format!(
                    "{need}{of_team}; {machine} signs only for team {}",
                    teams.join(", ")
                ),
                format!(
                    "on {machine}: add an Apple Development certificate{of_team} (Xcode > Settings > Accounts > Manage Certificates), or set [signing].team in .kr.toml to the app's team"
                ),
            )];
        }
        let reasons: Vec<String> = self
            .attempts
            .iter()
            .filter_map(|a| {
                a.result
                    .as_ref()
                    .err()
                    .map(|e| format!("{}: {e}", a.identity))
            })
            .collect();
        let locked = if reasons
            .iter()
            .any(|r| r.contains("errSecInternalComponent"))
        {
            " (the login keychain is locked in this session)"
        } else {
            ""
        };
        vec![Problem::new(
            "signing_unavailable",
            format!(
                "{need}; {machine} cannot sign in this session: identity {}{locked}",
                reasons.join("; ")
            ),
            format!(
                "on {machine}: make the login keychain usable in SSH sessions (unlocked at SSH login), or run this in a login session on {machine}; kr does not unlock keychains"
            ),
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real output of `ssh work` with two Apple Development identities, 2026-10-15
    // (fingerprints replaced).
    const WORK: &str = "identity 1 AAAA1111 signed team=K4AXH95DCF\nidentity 2 BBBB2222 signed team=K4AXH95DCF\nidentities=2\n";
    const LOCKED: &str = "identity 1 AAAA1111 failed errSecInternalComponent\nidentities=1\n";

    #[test]
    fn a_machine_that_signs_for_the_team_has_no_problem() {
        let trial = TrialSign::parse(WORK);
        assert_eq!(trial.attempts.len(), 2);
        assert_eq!(trial.attempts[0].identity, "#1 (AAAA1111)");
        assert!(
            trial
                .problems("work", "`launch`", Some("K4AXH95DCF"), "")
                .is_empty()
        );
        assert!(trial.problems("work", "`launch`", None, "").is_empty());
    }

    #[test]
    fn a_locked_keychain_says_so_and_what_to_do_on_that_machine() {
        let p = TrialSign::parse(LOCKED).problems("work", "`launch`", Some("K4AXH95DCF"), "");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].code, "signing_unavailable");
        assert_eq!(
            p[0].message,
            "`launch` needs a developer certificate; work cannot sign in this session: identity #1 (AAAA1111): errSecInternalComponent (the login keychain is locked in this session)"
        );
        assert!(
            p[0].next
                .starts_with("on work: make the login keychain usable in SSH sessions")
        );
        assert!(p[0].next.ends_with("kr does not unlock keychains"));
    }

    #[test]
    fn no_identity_and_another_team_are_told_apart() {
        let none = TrialSign::parse("identities=0\n").problems("work", "`x`", Some("T1"), "");
        assert_eq!(none[0].code, "signing_no_identity");
        assert!(none[0].next.contains("certificate of team T1"));
        let other = TrialSign::parse(WORK).problems("work", "`x`", Some("T1"), "");
        assert_eq!(other[0].code, "signing_wrong_team");
        assert!(
            other[0]
                .message
                .ends_with("work signs only for team K4AXH95DCF")
        );
    }

    #[test]
    fn a_trial_sign_that_did_not_finish_gives_its_last_error_line() {
        let p = TrialSign::parse("").problems("this Mac", "`x`", None, "\nsh: mktemp: failed\n");
        assert_eq!(p[0].code, "signing_unavailable");
        assert!(p[0].message.ends_with("did not finish: sh: mktemp: failed"));
    }
}
