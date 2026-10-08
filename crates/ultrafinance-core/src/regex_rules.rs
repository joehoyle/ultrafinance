//! Open Enrichment patterns run on original descriptors, preserving punctuation
//! and inline flags. Rust's bounded, non-backtracking engine deliberately skips
//! unsupported PCRE constructs rather than rewriting their meaning.
use regex::{Regex, RegexBuilder};
use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
};

type Cache = VecDeque<(String, Option<Regex>)>;

fn compiled(pattern: &str) -> Option<Regex> {
    if pattern.trim().is_empty() || pattern.len() > 4096 {
        return None;
    }
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(VecDeque::new()))
        .lock()
        .ok()?;
    if let Some(index) = cache.iter().position(|(key, _)| key == pattern) {
        let entry = cache.remove(index)?;
        let result = entry.1.clone();
        cache.push_back(entry);
        return result;
    }
    let regex = RegexBuilder::new(pattern)
        .size_limit(1024 * 1024)
        .build()
        .ok()
        .filter(|regex| !regex.is_match(""));
    if cache.len() >= 512 {
        cache.pop_front();
    }
    cache.push_back((pattern.to_owned(), regex.clone()));
    regex
}

// Common global processor codes from Open Enrichment's CC0 payment_processors.csv.
// Require the separator, so merchant names starting with these letters survive.
const PROCESSORS: &[(&str, char)] = &[
    ("SQ", '*'),
    ("SMP", '*'),
    ("LSP", '*'),
    ("LIGHTSPEED", '*'),
    ("CKO", '*'),
    ("FSPRG", '*'),
    ("FS", '*'),
    ("VIVA", '*'),
    ("PAYPAL", '*'),
    ("PP", '*'),
    ("SP", '*'),
    ("VISA", '-'),
    ("LS", ' '),
    ("SUMUP", '*'),
    ("Zettle_", '*'),
    ("EB", '*'),
    ("PADDLE.NET", '*'),
    ("BT", '*'),
    ("3CPAYMENT", '*'),
    ("WINDCAVE", '*'),
    ("UZR", '*'),
    ("NET", '*'),
    ("FH", '*'),
    ("WEB", '*'),
];

fn strip_processors(mut text: &str) -> &str {
    text = text.trim_start();
    loop {
        let stripped = PROCESSORS.iter().find_map(|(code, separator)| {
            let prefix = text.get(..code.len())?;
            if !prefix.eq_ignore_ascii_case(code) {
                return None;
            }
            let rest = &text[code.len()..];
            let rest = if *separator == ' ' {
                rest
            } else {
                rest.trim_start()
            };
            rest.strip_prefix(*separator).map(str::trim_start)
        });
        match stripped {
            Some(rest) => text = rest,
            None => return text,
        }
    }
}

pub(crate) fn match_length(pattern: &str, description: &str) -> Option<usize> {
    let regex = compiled(pattern)?;
    // Try raw text first, as upstream does, then the processor-stripped form.
    [description, strip_processors(description)]
        .into_iter()
        .find_map(|text| {
            regex
                .find(text)
                .filter(|m| !m.is_empty())
                .map(|m| m.as_str().chars().count())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descriptors_flags_boundaries_and_processors() {
        assert_eq!(
            match_length(r"(?i)^7-ELEVEN\b", "sq * PAYPAL * 7-eleven #999"),
            Some(8)
        );
        assert_eq!(match_length(r"(?i)^ALDI\b", "ALDINGTON"), None);
        assert_eq!(match_length(r"(?i)^SPAR\b", "SPAR 123"), Some(4));
        assert_eq!(match_length(r"^TIE(?: |\s)M", "TIE ME UP"), Some(5));
        assert_eq!(match_length(r"^TIE ME UP", "TIE ME UP"), Some(9));
        assert_eq!(match_length(r"(?i)^CAFÉ", "CAFÉ 123"), Some(4));
    }
    #[test]
    fn invalid_unsupported_empty_and_oversized_patterns_are_ignored() {
        for pattern in ["", "[", "(?=FOO)FOO", "(FOO)\\1", ".*", "^$"] {
            assert_eq!(match_length(pattern, "FOO"), None);
        }
        assert_eq!(match_length(&"X".repeat(4097), "X"), None);
    }
}
