//! Building a Windows command line for `CreateProcessAsUserW`.
//!
//! Quoting follows the documented parsing rules used by the Microsoft C
//! runtime and `CommandLineToArgvW`: arguments containing whitespace or
//! quotes are wrapped in quotes; quotes are escaped with a backslash; and a
//! run of backslashes is doubled only when it precedes a quote (or the
//! closing quote).

/// Quotes a single argument (if needed) and appends it to `out`.
pub fn push_arg(out: &mut String, arg: &str) {
    let needs_quotes = arg.is_empty()
        || arg
            .chars()
            .any(|c| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '"'));
    if !needs_quotes {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
}

/// `exe` followed by `args`, each quoted as needed, separated by spaces.
pub fn build(exe: &str, args: &[String]) -> String {
    let mut out = String::new();
    // The program name is parsed by different rules (no escapes), so it is
    // always quoted and must not contain a quote itself.
    out.push('"');
    out.push_str(exe);
    out.push('"');
    for a in args {
        out.push(' ');
        push_arg(&mut out, a);
    }
    out
}
