//! Deterministic spoken-emoji replacement. Short utterances skip LLM
//! cleanup entirely (`cleanupSkipShort`), which silently disabled the
//! prompt's emoji rule for exactly the quick exclamations people say.
//! This table runs offline on every path, before any model sees the text.

/// phrase (lowercase, matched without the trailing word "emoji") -> char.
/// Longer phrases are matched first where one is a prefix of another.
const PHRASES: &[(&str, &str)] = &[
    ("red heart", "❤️"),
    ("heart", "❤️"),
    ("thumbs up", "👍"),
    ("thumbs down", "👎"),
    ("hundred", "💯"),
    ("100", "💯"),
    ("skull", "💀"),
    ("fire", "🔥"),
    ("party", "🎉"),
    ("tada", "🎉"),
    ("clap", "👏"),
    ("pray", "🙏"),
    ("rocket", "🚀"),
    ("eyes", "👀"),
    ("joy", "😂"),
    ("laughing", "😂"),
    ("crying", "😢"),
    ("sob", "😭"),
    ("thinking", "🤔"),
    ("wink", "😉"),
    ("star", "⭐"),
    ("sparkles", "✨"),
    ("sparkle", "✨"),
    ("checkmark", "✅"),
    ("check box", "✅"),
    ("cross mark", "❌"),
    ("warning", "⚠️"),
    ("light bulb", "💡"),
    ("bulb", "💡"),
    ("lock", "🔒"),
    ("key", "🔑"),
    ("coffee", "☕"),
    ("pizza", "🍕"),
    ("ghost", "👻"),
    ("alien", "👽"),
    ("robot", "🤖"),
    ("sun", "☀️"),
    ("moon", "🌙"),
    ("lightning", "⚡"),
    ("zap", "⚡"),
    ("music", "🎵"),
    ("crown", "👑"),
    ("money bag", "💰"),
    ("money", "💰"),
    ("gem", "💎"),
    ("diamond", "💎"),
    ("wave", "👋"),
    ("muscle", "💪"),
    ("ok hand", "👌"),
    ("shushing face", "🤫"),
];

struct Needle {
    /// Full spoken pattern, e.g. "skull emoji".
    full: String,
    replacement: &'static str,
}

fn needles() -> Vec<Needle> {
    let mut v: Vec<Needle> = PHRASES
        .iter()
        .map(|(phrase, emoji)| Needle {
            full: format!("{phrase} emoji"),
            replacement: emoji,
        })
        .collect();
    v.sort_by_key(|n| std::cmp::Reverse(n.full.len()));
    v
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

/// Replaces every `<phrase> emoji` occurrence with the emoji itself.
/// Matching is case-insensitive; a match must start and end at word
/// boundaries so ordinary sentences never get mangled.
pub fn apply(text: &str) -> String {
    let needles = needles();
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let at_boundary = i == 0 || !is_word_byte(bytes[i - 1]);
        let mut matched = false;
        if at_boundary {
            for needle in &needles {
                let end = i + needle.full.len();
                if end <= bytes.len()
                    && text.is_char_boundary(end)
                    && text[i..end].eq_ignore_ascii_case(&needle.full)
                {
                    let after_ok = end == bytes.len() || !is_word_byte(bytes[end]);
                    if after_ok {
                        out.push_str(needle.replacement);
                        i = end;
                        matched = true;
                        break;
                    }
                }
            }
        }
        if !matched {
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_common_requests() {
        assert_eq!(apply("yo skull emoji lol"), "yo 💀 lol");
        assert_eq!(apply("That's FIRE EMOJI fr"), "That's 🔥 fr");
        assert_eq!(apply("add a party emoji!"), "add a 🎉!");
        assert_eq!(apply("done, thumbs up emoji."), "done, 👍.");
        assert_eq!(apply("rate it 100 emoji"), "rate it 💯");
    }

    #[test]
    fn longer_phrases_win_over_prefixes() {
        assert_eq!(apply("red heart emoji"), "❤️");
        assert_eq!(apply("ok hand emoji"), "👌");
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        assert_eq!(apply("the skull is spooky"), "the skull is spooky");
        assert_eq!(apply("emoji"), "emoji");
        assert_eq!(apply("fire drill at noon"), "fire drill at noon");
        assert_eq!(
            apply("hello world this is fine"),
            "hello world this is fine"
        );
    }

    #[test]
    fn handles_multibyte_text_safely() {
        assert_eq!(apply("café skull emoji ☕"), "café 💀 ☕");
        assert_eq!(apply("🎉 already there"), "🎉 already there");
    }

    #[test]
    fn multiple_replacements_in_one_pass() {
        assert_eq!(apply("fire emoji then skull emoji"), "🔥 then 💀");
    }
}
