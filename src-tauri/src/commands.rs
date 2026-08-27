use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

/// Voice commands recognized at the start of a dictation. Executed instead
/// of being pasted when command mode is enabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    OpenUrl(String),
    Copy(String),
    /// Remove the most recent dictation from the page (synthesized undo).
    ScratchThat,
}

const SITES: &[(&str, &str)] = &[
    ("youtube", "https://youtube.com"),
    ("github", "https://github.com"),
    ("gmail", "https://mail.google.com"),
    ("google docs", "https://docs.google.com"),
    ("google drive", "https://drive.google.com"),
    ("notion", "https://notion.so"),
    ("slack", "https://app.slack.com"),
    ("twitter", "https://x.com"),
    ("x", "https://x.com"),
    ("wikipedia", "https://wikipedia.org"),
    ("linkedin", "https://linkedin.com"),
];

pub fn is_enabled(db: &crate::store::Store) -> bool {
    db.get_setting("commandMode")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<bool>(&v).ok())
        .unwrap_or(true)
}

/// Parses a cleaned dictation into a command. Returns None when the text is
/// ordinary prose that should be pasted normally.
pub fn parse(text: &str) -> Option<Command> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return None;
    }

    for prefix in ["search for ", "search ", "google "] {
        if let Some(rest) = strip_prefix_ci(trimmed, prefix) {
            let query = rest.trim().trim_end_matches(['.', '!', '?']);
            if !query.is_empty() {
                return Some(Command::OpenUrl(format!(
                    "https://www.google.com/search?q={}",
                    url_encode(query)
                )));
            }
        }
    }

    if let Some(rest) = strip_prefix_ci(trimmed, "copy ") {
        let payload = rest.trim().to_string();
        if !payload.is_empty() {
            return Some(Command::Copy(payload));
        }
    }

    let unpunctuated = trimmed.trim_end_matches(['.', '!', '?']);
    for phrase in ["scratch that", "scratch this", "undo that", "undo this"] {
        if unpunctuated.eq_ignore_ascii_case(phrase) {
            return Some(Command::ScratchThat);
        }
    }

    if let Some(rest) = strip_prefix_ci(trimmed, "open ") {
        let site = rest.trim().trim_end_matches(['.', '!', '?']);
        let site_key = site.to_lowercase();
        if !site.is_empty() {
            // Check named destinations before the single-token domain gate;
            // otherwise entries such as "google docs" can never match.
            if let Some((_, url)) = SITES.iter().find(|(name, _)| *name == site_key) {
                return Some(Command::OpenUrl(url.to_string()));
            }
            if !site.chars().any(char::is_whitespace)
                && site.contains('.')
                && !site.starts_with('.')
            {
                let url = if site_key.starts_with("http://") || site_key.starts_with("https://") {
                    site.to_string()
                } else {
                    format!("https://{site}")
                };
                return Some(Command::OpenUrl(url));
            }
        }
    }

    None
}

pub fn execute(app: &AppHandle, command: &Command) -> Result<(), String> {
    match command {
        Command::OpenUrl(url) => app
            .opener()
            .open_url(url, None::<&str>)
            .map_err(|e| format!("failed to open {url}: {e}")),
        Command::Copy(text) => arboard::Clipboard::new()
            .and_then(|mut cb| cb.set_text(text.to_string()))
            .map_err(|e| format!("clipboard unavailable: {e}")),
        Command::ScratchThat => crate::scratch_last(),
    }
}

pub fn describe(command: &Command) -> String {
    match command {
        Command::OpenUrl(url) => format!("Opening {url}"),
        Command::Copy(_) => "Copied to clipboard".to_string(),
        Command::ScratchThat => "Removed the last dictation".to_string(),
    }
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &text[prefix.len()..])
}

fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_variants_case_insensitive() {
        assert_eq!(
            parse("Search for rust async books"),
            Some(Command::OpenUrl(
                "https://www.google.com/search?q=rust%20async%20books".to_string()
            ))
        );
        assert_eq!(
            parse("GOOGLE hello world"),
            Some(Command::OpenUrl(
                "https://www.google.com/search?q=hello%20world".to_string()
            ))
        );
    }

    #[test]
    fn search_encodes_specials_and_trims_punctuation() {
        assert_eq!(
            parse("search c++ vs rust?"),
            Some(Command::OpenUrl(
                "https://www.google.com/search?q=c%2B%2B%20vs%20rust".to_string()
            ))
        );
    }

    #[test]
    fn copies_rest_of_text_verbatim() {
        assert_eq!(
            parse("copy jon@example.com and friends"),
            Some(Command::Copy("jon@example.com and friends".to_string()))
        );
    }

    #[test]
    fn opens_known_sites_and_domains() {
        assert_eq!(
            parse("Open YouTube."),
            Some(Command::OpenUrl("https://youtube.com".to_string()))
        );
        assert_eq!(
            parse("open news.ycombinator.com"),
            Some(Command::OpenUrl("https://news.ycombinator.com".to_string()))
        );
        assert_eq!(
            parse("open google docs"),
            Some(Command::OpenUrl("https://docs.google.com".to_string()))
        );
        assert_eq!(
            parse("open example.com/CaseSensitivePath"),
            Some(Command::OpenUrl(
                "https://example.com/CaseSensitivePath".to_string()
            ))
        );
    }

    #[test]
    fn prose_is_not_a_command() {
        assert_eq!(parse("please open the door for me"), None);
        assert_eq!(parse("I want to copy your style"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("just talking about how to open things"), None);
    }

    #[test]
    fn scratch_that_variants_parse() {
        assert_eq!(parse("Scratch that"), Some(Command::ScratchThat));
        assert_eq!(parse("undo that."), Some(Command::ScratchThat));
        assert_eq!(parse("please scratch that"), None);
    }

    #[test]
    fn multi_line_is_never_a_command() {
        assert_eq!(parse("open\nyoutube"), None);
    }
}
