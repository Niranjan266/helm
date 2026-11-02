//! Command-line handling for `Helm.exe`, including what a second launch asks the running copy
//! to do (the single-instance plugin forwards the second process's argv to us).

/// Start without showing the window (used by Start with Windows).
pub const HIDDEN: &str = "--hidden";
/// Older spelling used by `helm-agent --background`; treated like `--hidden`.
pub const BACKGROUND: &str = "--background";
/// Open straight to a fresh pairing QR.
pub const PAIR: &str = "--pair";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LaunchArgs {
    pub hidden: bool,
    pub pair: bool,
}

/// Parse a full argv (the first item is the program and is skipped). Unknown arguments are
/// ignored so a stale shortcut never stops Helm from starting.
pub fn parse<I, S>(argv: I) -> LaunchArgs
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = LaunchArgs::default();
    for arg in argv.into_iter().skip(1) {
        match arg.as_ref().to_ascii_lowercase().as_str() {
            HIDDEN | BACKGROUND => out.hidden = true,
            PAIR => out.pair = true,
            _ => {}
        }
    }
    // Asking for a QR means the user wants to see the window.
    if out.pair {
        out.hidden = false;
    }
    out
}

/// What the running copy does when Helm is started again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecondLaunch {
    /// Started hidden (e.g. the Run key fired while Helm was already open): stay as we are.
    Ignore,
    /// Bring the window to the front.
    Show,
    /// Bring the window to the front on the Pair page with a fresh code.
    ShowPairing,
}

pub fn second_launch<I, S>(argv: I) -> SecondLaunch
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args = parse(argv);
    if args.pair {
        SecondLaunch::ShowPairing
    } else if args.hidden {
        SecondLaunch::Ignore
    } else {
        SecondLaunch::Show
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_launch_shows_window() {
        assert_eq!(parse(["Helm.exe"]), LaunchArgs::default());
        assert_eq!(parse(Vec::<String>::new()), LaunchArgs::default());
    }

    #[test]
    fn hidden_and_legacy_background() {
        assert!(parse(["Helm.exe", "--hidden"]).hidden);
        assert!(parse(["Helm.exe", "--BACKGROUND"]).hidden);
        // The program name itself is never treated as a flag.
        assert!(!parse(["--hidden"]).hidden);
    }

    #[test]
    fn pair_wins_over_hidden_and_unknown_is_ignored() {
        let a = parse(["Helm.exe", "--hidden", "--pair", "--whatever", "x"]);
        assert_eq!(
            a,
            LaunchArgs {
                hidden: false,
                pair: true
            }
        );
    }

    #[test]
    fn second_launch_actions() {
        assert_eq!(second_launch(["Helm.exe"]), SecondLaunch::Show);
        assert_eq!(second_launch(["Helm.exe", "--hidden"]), SecondLaunch::Ignore);
        assert_eq!(
            second_launch(["Helm.exe", "--pair"]),
            SecondLaunch::ShowPairing
        );
        assert_eq!(
            second_launch(["C:\\Apps\\Helm.exe", "--hidden", "--pair"]),
            SecondLaunch::ShowPairing
        );
    }
}
