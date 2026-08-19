//! What the beta installer checks and decides.
//!
//! Split out of the binary so it is testable: the decisions here (is this
//! dependency satisfied? what should the user be told? what needs installing?)
//! are pure functions over facts, and the binary's job is only to GATHER those
//! facts and act on the answers.
//!
//! Why an .exe at all, rather than the PowerShell script this replaces: a
//! `.ps1` cannot be relied on to run from a double-click. Three separate
//! things stop it and all three are SILENT -- the window appears and closes
//! with no error:
//!
//!   1. `.ps1` frequently has no file association, so double-clicking opens an
//!      editor or does nothing.
//!   2. A fresh account's execution policy is `Restricted`, which blocks it.
//!   3. Anything extracted from a downloaded `.zip` with Explorer carries Mark
//!      of the Web, which PowerShell refuses to run even under `RemoteSigned`.
//!
//! Measured, not assumed: on the machine this was written, `.ps1` had no
//! association at all, `CurrentUser` policy was Undefined, and Explorer's
//! extractor propagated `ZoneId=3` to every extracted file.
//!
//! An `.exe` has none of those problems.

/// How much a missing dependency actually costs the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Without this, panefx cannot run at all.
    Required,
    /// Without this, a NAMED part of panefx does nothing. Everything else
    /// works, so this must not read like a failure.
    PartLost,
    /// Nice to have.
    Optional,
}

/// One thing the installer checks for.
#[derive(Debug, Clone)]
pub struct Check {
    pub name: &'static str,
    pub need: Need,
    /// What the user loses without it, in their words rather than ours.
    pub why: &'static str,
    /// winget package id, when winget can install it.
    pub winget: Option<&'static str>,
    pub url: &'static str,
    pub found: bool,
    /// What was found instead, when that is the interesting part -- the font
    /// check reports the substituted face here.
    pub detail: Option<String>,
}

impl Check {
    /// Should the installer offer to install this?
    pub fn wants_install(&self) -> bool {
        !self.found && self.need != Need::Optional
    }

    /// One line for the summary.
    pub fn status_line(&self) -> String {
        let mark = if self.found { "[ok]  " } else { "[need]" };
        match (&self.detail, self.found) {
            (Some(d), false) => format!("{mark} {:<22} {d}", self.name),
            (Some(d), true) => format!("{mark} {:<22} {d}", self.name),
            (None, true) => format!("{mark} {:<22} found", self.name),
            (None, false) => format!("{mark} {:<22} not found", self.name),
        }
    }
}

/// Can panefx run at all, given these results?
///
/// Deliberately distinct from "everything passed": a missing GlazeWM costs the
/// pane effects and nothing else, and telling someone their install failed when
/// the wallpaper effects work fine would be a lie.
pub fn can_run(checks: &[Check]) -> bool {
    !checks
        .iter()
        .any(|c| c.need == Need::Required && !c.found)
}

/// What the user should be told once the checks are done.
pub fn verdict(checks: &[Check]) -> &'static str {
    if !can_run(checks) {
        return "panefx cannot run on this machine.";
    }
    let lost: Vec<_> = checks
        .iter()
        .filter(|c| c.need == Need::PartLost && !c.found)
        .collect();
    if lost.is_empty() {
        "Everything panefx needs is present."
    } else {
        "panefx will run, but part of it is switched off (see above)."
    }
}

/// Did GDI give us the font we asked for, or silently substitute another?
///
/// THE check nobody thinks to make. `CreateFontW` does not fail on a missing
/// font -- it returns a valid handle for a substitute, so every effect renders
/// in the wrong typeface with no error anywhere. Only reading the face back
/// tells the truth.
pub fn font_is_real(requested: &str, resolved: Option<&str>) -> bool {
    match resolved {
        // A face name comparison, case-insensitively: GDI echoes the name back
        // with its own capitalisation.
        Some(got) => got.eq_ignore_ascii_case(requested),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &'static str, need: Need, found: bool) -> Check {
        Check {
            name,
            need,
            why: "",
            winget: None,
            url: "",
            found,
            detail: None,
        }
    }

    #[test]
    fn a_missing_required_thing_stops_the_install() {
        let c = vec![check("Windows 10", Need::Required, false)];
        assert!(!can_run(&c));
        assert_eq!(verdict(&c), "panefx cannot run on this machine.");
    }

    #[test]
    fn a_missing_glazewm_does_not_read_as_failure() {
        // GlazeWM missing costs the PANE effects and nothing else. The
        // wallpaper still works, so calling this a failed install would be
        // wrong -- and would send a tester chasing a bug that is not there.
        let c = vec![
            check("Windows 10", Need::Required, true),
            check("GlazeWM", Need::PartLost, false),
        ];
        assert!(can_run(&c));
        assert_eq!(
            verdict(&c),
            "panefx will run, but part of it is switched off (see above)."
        );
    }

    #[test]
    fn a_missing_optional_thing_is_not_offered_for_install() {
        // Alacritty. Offering to install a terminal to someone who did not ask
        // for one is presumptuous.
        assert!(!check("Alacritty", Need::Optional, false).wants_install());
        assert!(check("GlazeWM", Need::PartLost, false).wants_install());
        assert!(!check("GlazeWM", Need::PartLost, true).wants_install());
    }

    #[test]
    fn everything_present_says_so_plainly() {
        let c = vec![
            check("Windows 10", Need::Required, true),
            check("GlazeWM", Need::PartLost, true),
            check("Alacritty", Need::Optional, false),
        ];
        assert_eq!(verdict(&c), "Everything panefx needs is present.");
    }

    #[test]
    fn a_substituted_font_is_not_the_font() {
        // The real measured case: asking for BigBlueTerm437 on a machine
        // without it returns "Arial", successfully.
        assert!(!font_is_real(
            "BigBlueTerm437 Nerd Font Mono",
            Some("Arial")
        ));
        assert!(font_is_real(
            "BigBlueTerm437 Nerd Font Mono",
            Some("BigBlueTerm437 Nerd Font Mono")
        ));
        // GDI may echo different capitalisation; that is still the font.
        assert!(font_is_real("BigBlueTerm437 Nerd Font Mono", Some("bigblueterm437 nerd font mono")));
        assert!(!font_is_real("BigBlueTerm437 Nerd Font Mono", None));
    }
}
