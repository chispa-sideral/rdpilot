//! Line lexing for hosts files and `-o` options.
//!
//! A line is `Keyword value` or `Keyword=value` (optional blanks around one
//! `=`). Blank lines and lines starting with `#` are ignored. Values:
//! `"double quotes"` group words (`\"` and `\\` are the only escapes inside
//! them) and a `#` at the start of a word ends the line. `PasswordCommand`
//! takes the rest of the line verbatim, unless the whole rest is one
//! double-quoted string, which is unquoted.

/// Split a line into `(keyword, rest)`. `None` for blank/comment lines.
pub(crate) fn split_line(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let (keyword, rest) = line.split_at(end);
    let rest = rest.trim_start();
    let rest = match rest.strip_prefix('=') {
        Some(after) => after.trim_start(),
        None => rest,
    };
    Some((keyword, rest.trim_end()))
}

/// Split the rest of a line into words.
pub(crate) fn words(rest: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut in_quote = false;
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quote {
            match c {
                '\\' if matches!(chars.peek(), Some('"' | '\\')) => {
                    cur.push(chars.next().unwrap_or('\\'));
                }
                '"' => in_quote = false,
                _ => cur.push(c),
            }
        } else if c == '"' {
            in_quote = true;
            in_word = true;
        } else if c.is_whitespace() {
            if in_word {
                out.push(std::mem::take(&mut cur));
                in_word = false;
            }
        } else if c == '#' && !in_word {
            break;
        } else {
            cur.push(c);
            in_word = true;
        }
    }
    if in_quote {
        return Err("unterminated double quote".to_owned());
    }
    if in_word {
        out.push(cur);
    }
    Ok(out)
}

/// The value of a command keyword: the whole rest, unquoted when it is one
/// double-quoted string.
pub(crate) fn command_value(rest: &str) -> String {
    let Some(body) = rest.strip_prefix('"') else {
        return rest.to_owned();
    };
    let mut out = String::new();
    let mut chars = body.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' if matches!(chars.peek(), Some((_, '"' | '\\'))) => {
                if let Some((_, n)) = chars.next() {
                    out.push(n);
                }
            }
            '"' => {
                return if i + 1 == body.len() {
                    out
                } else {
                    rest.to_owned()
                };
            }
            _ => out.push(c),
        }
    }
    rest.to_owned()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn keyword_forms() {
        assert_eq!(split_line("  Port 3390"), Some(("Port", "3390")));
        assert_eq!(split_line("Port=3390"), Some(("Port", "3390")));
        assert_eq!(split_line("Port = 3390  "), Some(("Port", "3390")));
        assert_eq!(split_line("port\t3390"), Some(("port", "3390")));
        assert_eq!(split_line("# comment"), None);
        assert_eq!(split_line("   "), None);
        assert_eq!(split_line("Host"), Some(("Host", "")));
    }

    #[test]
    fn words_handle_quotes_and_comments() {
        assert_eq!(words("a b  c").unwrap(), ["a", "b", "c"]);
        assert_eq!(words("\"a b\" c").unwrap(), ["a b", "c"]);
        assert_eq!(words("a # trailing").unwrap(), ["a"]);
        assert_eq!(words("pa#ss").unwrap(), ["pa#ss"]);
        assert_eq!(words("\"a \\\"q\\\" \\\\\"").unwrap(), ["a \"q\" \\"]);
        assert_eq!(words("\"\"").unwrap(), [""]);
        assert!(words("\"open").is_err());
    }

    #[test]
    fn command_value_forms() {
        assert_eq!(command_value("printenv X"), "printenv X");
        assert_eq!(command_value("\"printenv X\""), "printenv X");
        assert_eq!(command_value("\"a\" | tr"), "\"a\" | tr");
        assert_eq!(command_value("echo a # b"), "echo a # b");
        assert_eq!(command_value("\"open"), "\"open");
    }
}
