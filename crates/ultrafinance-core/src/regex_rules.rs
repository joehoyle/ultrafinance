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
#[derive(Debug, serde::Serialize)]
pub struct ProcessorFormat {
    pub code: &'static str,
    pub separator: char,
    pub processor: &'static str,
}

const PROCESSORS: &[ProcessorFormat] = &[
    ProcessorFormat {
        code: "SQ",
        separator: '*',
        processor: "Square",
    },
    ProcessorFormat {
        code: "TST",
        separator: '*',
        processor: "Toast",
    },
    ProcessorFormat {
        code: "GOSQ.COM",
        separator: ' ',
        processor: "Square",
    },
    ProcessorFormat {
        code: "SMP",
        separator: '*',
        processor: "SMP",
    },
    ProcessorFormat {
        code: "LSP",
        separator: '*',
        processor: "Lightspeed",
    },
    ProcessorFormat {
        code: "LIGHTSPEED",
        separator: '*',
        processor: "Lightspeed",
    },
    ProcessorFormat {
        code: "CKO",
        separator: '*',
        processor: "Checkout.com",
    },
    ProcessorFormat {
        code: "FSPRG",
        separator: '*',
        processor: "FastSpring",
    },
    ProcessorFormat {
        code: "FS",
        separator: '*',
        processor: "FastSpring",
    },
    ProcessorFormat {
        code: "VIVA",
        separator: '*',
        processor: "VIVA",
    },
    ProcessorFormat {
        code: "PAYPAL",
        separator: '*',
        processor: "PayPal",
    },
    ProcessorFormat {
        code: "PP",
        separator: '*',
        processor: "PayPal",
    },
    ProcessorFormat {
        code: "SP",
        separator: '*',
        processor: "Shopify",
    },
    ProcessorFormat {
        code: "VISA",
        separator: '-',
        processor: "VISA",
    },
    ProcessorFormat {
        code: "LS",
        separator: ' ',
        processor: "Lightspeed",
    },
    ProcessorFormat {
        code: "SUMUP",
        separator: '*',
        processor: "SumUp",
    },
    ProcessorFormat {
        code: "Zettle_",
        separator: '*',
        processor: "Zettle_",
    },
    ProcessorFormat {
        code: "EB",
        separator: '*',
        processor: "EB",
    },
    ProcessorFormat {
        code: "PADDLE.NET",
        separator: '*',
        processor: "Paddle",
    },
    ProcessorFormat {
        code: "BT",
        separator: '*',
        processor: "BT",
    },
    ProcessorFormat {
        code: "3CPAYMENT",
        separator: '*',
        processor: "3CPAYMENT",
    },
    ProcessorFormat {
        code: "WINDCAVE",
        separator: '*',
        processor: "WINDCAVE",
    },
    ProcessorFormat {
        code: "UZR",
        separator: '*',
        processor: "UZR",
    },
    ProcessorFormat {
        code: "NET",
        separator: '*',
        processor: "NET",
    },
    ProcessorFormat {
        code: "FH",
        separator: '*',
        processor: "FH",
    },
    ProcessorFormat {
        code: "WEB",
        separator: '*',
        processor: "WEB",
    },
];

pub(crate) fn processor_formats() -> &'static [ProcessorFormat] {
    PROCESSORS
}

/// A separator is mandatory: SQUID, SPAR and standalone LS are not processors.
pub(crate) fn processor_prefix(text: &str) -> Option<(&'static str, &str)> {
    let text = text.trim_start();
    PROCESSORS.iter().find_map(|rule| {
        let code = rule.code;
        let separator = rule.separator;
        if !text.get(..code.len())?.eq_ignore_ascii_case(code) {
            return None;
        }
        let rest = &text[code.len()..];
        let rest = if separator == ' ' {
            rest
        } else {
            rest.trim_start()
        };
        let rest = rest.strip_prefix(separator)?.trim_start();
        if rest.is_empty() {
            return None;
        }
        Some((rule.processor, rest))
    })
}

fn strip_processors(mut text: &str) -> &str {
    text = text.trim_start();
    while let Some((_, rest)) = processor_prefix(text) {
        text = rest;
    }
    text
}

pub(crate) fn valid_pattern(pattern: &str) -> bool {
    compiled(pattern).is_some()
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
        assert_eq!(match_length(r"(?i)^ALDI\b", "TST * ALDI"), Some(4));
        assert_eq!(match_length(r"(?i)^ALDI\b", "gosq.com ALDI"), Some(4));
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
