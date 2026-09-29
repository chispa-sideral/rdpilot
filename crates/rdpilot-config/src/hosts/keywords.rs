//! The keyword table: one static slice describing every supported hosts-file
//! keyword. Adding a keyword is adding a row.

/// How a keyword's value is read and validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A single non-empty word.
    Text,
    /// A TCP port, 1-65535.
    Port,
    /// `yes` or `no`.
    YesNo,
    /// The rest of the line, run through a shell.
    Command,
    /// One or more glob patterns naming files to read.
    Include,
}

/// A setting slot: the first value obtained for a slot wins. Keywords that
/// share a slot compete for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    HostName,
    Port,
    User,
    Domain,
    Credential,
    AcceptInvalidCerts,
    CuaEnabled,
    Include,
}

/// One row of the keyword table.
#[derive(Debug)]
pub(crate) struct Keyword {
    /// Canonical spelling (keywords match case-insensitively).
    pub name: &'static str,
    pub kind: Kind,
    pub slot: Slot,
    /// The value is a credential and is never printed.
    pub secret: bool,
}

pub(crate) const KEYWORDS: &[Keyword] = &[
    Keyword {
        name: "HostName",
        kind: Kind::Text,
        slot: Slot::HostName,
        secret: false,
    },
    Keyword {
        name: "Port",
        kind: Kind::Port,
        slot: Slot::Port,
        secret: false,
    },
    Keyword {
        name: "User",
        kind: Kind::Text,
        slot: Slot::User,
        secret: false,
    },
    Keyword {
        name: "Domain",
        kind: Kind::Text,
        slot: Slot::Domain,
        secret: false,
    },
    Keyword {
        name: "Password",
        kind: Kind::Text,
        slot: Slot::Credential,
        secret: true,
    },
    Keyword {
        name: "PasswordCommand",
        kind: Kind::Command,
        slot: Slot::Credential,
        secret: false,
    },
    Keyword {
        name: "AcceptInvalidCerts",
        kind: Kind::YesNo,
        slot: Slot::AcceptInvalidCerts,
        secret: false,
    },
    Keyword {
        name: "CuaEnabled",
        kind: Kind::YesNo,
        slot: Slot::CuaEnabled,
        secret: false,
    },
    Keyword {
        name: "Include",
        kind: Kind::Include,
        slot: Slot::Include,
        secret: false,
    },
];

pub(crate) const PORT: usize = 1;
pub(crate) const USER: usize = 2;
pub(crate) const DOMAIN: usize = 3;
pub(crate) const PASSWORD: usize = 4;

pub(crate) fn lookup(name: &str) -> Option<&'static Keyword> {
    KEYWORDS.iter().find(|k| k.name.eq_ignore_ascii_case(name))
}

pub(crate) fn index_of(kw: &Keyword) -> usize {
    KEYWORDS
        .iter()
        .position(|k| std::ptr::eq(k, kw))
        .unwrap_or(usize::MAX)
}

/// Validate and normalise a value for its keyword kind.
pub(crate) fn validate(kw: &Keyword, value: &str) -> Result<String, String> {
    match kw.kind {
        Kind::Text | Kind::Command | Kind::Include => {
            if value.is_empty() {
                Err(format!("{} needs a value", kw.name))
            } else {
                Ok(value.to_owned())
            }
        }
        Kind::Port => match value.parse::<u16>() {
            Ok(p) if p != 0 => Ok(p.to_string()),
            _ => Err(format!("{} must be a port number from 1 to 65535", kw.name)),
        },
        Kind::YesNo => match value.to_ascii_lowercase().as_str() {
            "yes" => Ok("yes".to_owned()),
            "no" => Ok("no".to_owned()),
            _ => Err(format!("{} must be yes or no", kw.name)),
        },
    }
}
