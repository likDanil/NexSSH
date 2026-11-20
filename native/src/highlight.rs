//! The 1.x word patterns, mapped to terminal columns while preserving ANSI colours.
use regex::Regex;
use std::sync::OnceLock;
include!(concat!(env!("OUT_DIR"), "/highlight_words.rs"));
pub const KINDS: [&str; 6] = ["error", "warning", "ok", "info", "debug", "address"];

pub fn find(text: &str) -> Vec<(usize, usize, usize)> {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        WORDS
            .iter()
            .map(|(_, pattern)| Regex::new(&format!(r"(?i)\b(?:{pattern})\b")).unwrap())
            .chain([Regex::new(r"(?i)[0-9a-f:.\-]+").unwrap()])
            .collect()
    });
    let mut found = vec![];
    for (kind, pattern) in patterns.iter().enumerate() {
        for m in pattern.find_iter(text) {
            if kind == 5 {
                let value = m.as_str().trim_matches(['.', '-']);
                let mac = value.len() == 17
                    && value.as_bytes().iter().enumerate().all(|(i, b)| {
                        if i % 3 == 2 {
                            *b == b':' || *b == b'-'
                        } else {
                            b.is_ascii_hexdigit()
                        }
                    })
                    && (value.matches(':').count() == 5 || value.matches('-').count() == 5);
                if !mac && value.parse::<std::net::IpAddr>().is_err() {
                    continue;
                }
            }
            let word = |c: char| c.is_alphanumeric() || c == '_';
            if text[..m.start()].chars().next_back().is_some_and(word)
                || text[m.end()..].chars().next().is_some_and(word)
            {
                continue;
            }
            found.push((m.start(), m.end(), kind));
        }
    }
    found.sort_unstable();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_original_words_and_valid_addresses() {
        let text = "[ERROR] errorCount Ошибкаerror failed OK 192.168.0.1 fe80::1 00:1a:2b:3c:4d:5e 999.1.2.3";
        let matches = find(text)
            .iter()
            .map(|(a, b, _)| &text[*a..*b])
            .collect::<Vec<_>>();
        assert_eq!(
            matches,
            [
                "ERROR",
                "failed",
                "OK",
                "192.168.0.1",
                "fe80::1",
                "00:1a:2b:3c:4d:5e"
            ]
        );
    }
}
