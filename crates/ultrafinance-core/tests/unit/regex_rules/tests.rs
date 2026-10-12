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
