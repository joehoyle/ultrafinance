//! Conservative bank-format hypotheses. Removed spans are clues, not facts.
use regex::Regex;
use std::sync::OnceLock;

struct Patterns {
    fields: Regex,
    metadata: Regex,
    wrapper: Regex,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Fields,
    Metadata,
    Wrapper,
}

// These definitions drive both parsing and the CLI inventory.
const RULES: &[(Kind, super::Format)] = &[
    (
        Kind::Fields,
        super::Format {
            id: "ach-fields",
            description: "Extract company text from multi-field ACH layouts; retain recipient and reference fields as clues.",
            pattern: Some(
                r"(?i)(?:ORIG CO NAME|ORIG ID|DESC DATE|CO ENTRY DESCR|IND ID|IND NAME|CO ID|INDN|DES|ID|SEC|TRACE|EED|TRN)\s*(?:#\s*)?:",
            ),
        },
    ),
    (
        Kind::Metadata,
        super::Format {
            id: "labelled-references",
            description: "Remove explicitly labelled references containing digits, preserving their original text as clues.",
            pattern: Some(
                r"(?i)\b(?:REF(?:ERENCE)?|AUTH|TRACE|ORDER|TRANSACTION|TXN|UTR)\s*(?:#|:|NO\.?\s*)\s*[A-Z0-9][A-Z0-9/-]*",
            ),
        },
    ),
    (
        Kind::Metadata,
        super::Format {
            id: "year-first-dates",
            description: "Recognize YYYY-MM-DD and YYYY/MM/DD numeric date spans.",
            pattern: Some(r"\b(?:19|20)\d{2}[-/](?:0[1-9]|1[0-2])[-/](?:0[1-9]|[12]\d|3[01])\b"),
        },
    ),
    (
        Kind::Metadata,
        super::Format {
            id: "month-first-dates",
            description: "Recognize MM/DD or MM-DD with optional two- or four-digit year; date order remains unverified.",
            pattern: Some(
                r"\b(?:0?[1-9]|1[0-2])[-/](?:0?[1-9]|[12]\d|3[01])(?:[-/](?:\d{4}|\d{2}))?\b",
            ),
        },
    ),
    (
        Kind::Metadata,
        super::Format {
            id: "day-first-dates",
            description: "Recognize DD/MM or DD-MM with optional two- or four-digit year; date order remains unverified.",
            pattern: Some(
                r"\b(?:0?[1-9]|[12]\d|3[01])[-/](?:0?[1-9]|1[0-2])(?:[-/](?:\d{4}|\d{2}))?\b",
            ),
        },
    ),
    (
        Kind::Metadata,
        super::Format {
            id: "north-american-phones",
            description: "Recognize punctuated North American phone numbers with optional country code; never infer geography from them.",
            pattern: Some(r"(?:\+?1[ .-]?)?(?:\([2-9]\d{2}\)[ .-]?|[2-9]\d{2}[.-])\d{3}[.-]\d{4}"),
        },
    ),
    (
        Kind::Wrapper,
        super::Format {
            id: "bank-wrappers",
            description: "Remove anchored card/POS/ACH transaction wrappers, including CHECKCARD MMDD.",
            pattern: Some(
                r"(?i)^(?:(?:CHECKCARD|CHECK CARD)\s+(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])|DEBIT CARD PURCHASE|CREDIT CARD PURCHASE|CHECKCARD|CHECK CARD|POS PURCHASE|POS DEBIT|CARD PURCHASE|ACH DEBIT|ACH CREDIT)(?:\s+|$)",
            ),
        },
    ),
];

pub(super) fn formats() -> impl Iterator<Item = super::Format> {
    RULES.iter().map(|(_, format)| *format)
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let compile = |kind| {
            let patterns: Vec<_> = RULES
                .iter()
                .filter(|(rule_kind, _)| *rule_kind == kind)
                .map(|(_, rule)| format!("(?:{})", rule.pattern.unwrap()))
                .collect();
            Regex::new(&patterns.join("|")).unwrap()
        };
        Patterns {
            fields: compile(Kind::Fields),
            metadata: compile(Kind::Metadata),
            wrapper: compile(Kind::Wrapper),
        }
    })
}

pub(super) fn merchant_text(original: &str) -> (String, Vec<String>) {
    let patterns = patterns();
    let mut clues = Vec::new();
    let fields: Vec<_> = patterns.fields.find_iter(original).collect();
    let mut text = original.to_owned();
    // Require a multi-field layout; DES: alone may be part of a merchant name.
    if fields.len() >= 2 {
        let origin = fields.iter().position(|field| {
            field
                .as_str()
                .to_ascii_uppercase()
                .starts_with("ORIG CO NAME")
        });
        if let Some(index) = origin {
            let field = &fields[index];
            let end = fields.get(index + 1).map_or(original.len(), |f| f.start());
            text = original[field.end()..end].trim().to_owned();
            if field.start() > 0 {
                clues.push(original[..field.start()].trim().to_owned());
            }
            if end < original.len() {
                clues.push(original[end..].trim().to_owned());
            }
        } else if fields.iter().any(|field| {
            matches!(
                field
                    .as_str()
                    .split(':')
                    .next()
                    .unwrap()
                    .trim()
                    .to_ascii_uppercase()
                    .as_str(),
                "DES" | "CO ID"
            )
        }) && fields.iter().any(|field| {
            field.as_str().to_ascii_uppercase().starts_with("INDN")
                || field.as_str().to_ascii_uppercase().starts_with("SEC")
        }) {
            let at = fields[0].start();
            text = original[..at].trim().to_owned();
            clues.push(original[at..].trim().to_owned());
        }
    }
    let mut cleaned = String::new();
    let mut at = 0;
    for span in patterns.metadata.find_iter(&text) {
        // Do not carve dates/phone numbers out of alphanumeric merchant names.
        let numeric = !span.as_str().chars().any(char::is_alphabetic);
        let boundary = |c: char| c.is_alphanumeric() || (numeric && matches!(c, '/' | '-' | '.'));
        if text[..span.start()]
            .chars()
            .next_back()
            .is_some_and(boundary)
            || text[span.end()..].chars().next().is_some_and(boundary)
            || (span.as_str().chars().any(char::is_alphabetic)
                && !span.as_str().chars().any(|c| c.is_ascii_digit()))
        {
            continue;
        }
        cleaned.push_str(&text[at..span.start()]);
        cleaned.push(' ');
        clues.push(span.as_str().to_owned());
        at = span.end();
    }
    cleaned.push_str(&text[at..]);
    let mut cleaned = cleaned.trim();
    while let Some(span) = patterns.wrapper.find(cleaned) {
        clues.push(span.as_str().trim().to_owned());
        cleaned = cleaned[span.end()..].trim_start();
    }
    // A descriptor containing only metadata yields no invented merchant.
    (
        cleaned.split_whitespace().collect::<Vec<_>>().join(" "),
        clues,
    )
}
