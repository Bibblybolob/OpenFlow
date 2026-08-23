//! Automatic vocabulary learning.
//!
//! After every successful cleanup pass we compare the raw speech-to-text
//! output against the LLM's polished rewrite. Segments that were rewritten
//! while staying phonetically similar (spelling fixes like "john smyth" to
//! "John Smythe") are treated as vocabulary candidates:
//!
//! - Proper-noun phrases (all tokens capitalized) are added to the dictionary
//!   silently, so Flow learns unique words and names automatically.
//! - Everything else (case-only fixes of jargon, partially-capitalized terms)
//!   lands in a review queue surfaced on the Dictionary page.
//!
//! Learning never blocks or fails a dictation: every error is swallowed and
//! logged, and per-session recording is capped.

use std::collections::HashSet;

use crate::store::{Result, Store};

/// At most this many new candidates are recorded per dictation.
const MAX_CANDIDATES_PER_SESSION: usize = 3;
/// Replacement segments wider than this on either side are treated as
/// rephrasing, not spelling fixes.
const MAX_SEGMENT_TOKENS: usize = 3;
/// Dictations longer than this many tokens are skipped (keeps the O(n*m)
/// diff bounded; polish input itself is capped at ~6000 chars).
const MAX_TOKENS_FOR_DIFF: usize = 400;
/// Joined-form edit-distance ratio above which a pair counts as rephrasing.
const MAX_SIMILARITY_RATIO: f64 = 0.45;
/// Tight-similarity threshold that qualifies a single lowercase token
/// (e.g. mangled jargon like "coober netes" to "kubernetes") for the queue.
const TIGHT_SIMILARITY_RATIO: f64 = 0.2;
/// Longest allowed single token inside a learned term.
const MAX_TERM_CHARS: usize = 30;

/// Words that must never enter the vocabulary: fillers, function words and
/// spoken formatting commands ("new paragraph", "bullet list", ...).
const STOPWORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "and",
    "or",
    "but",
    "nor",
    "so",
    "yet",
    "if",
    "then",
    "else",
    "when",
    "i",
    "im",
    "ive",
    "id",
    "you",
    "your",
    "youre",
    "we",
    "our",
    "ours",
    "they",
    "them",
    "their",
    "he",
    "him",
    "his",
    "she",
    "her",
    "hers",
    "it",
    "its",
    "this",
    "that",
    "these",
    "those",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "am",
    "do",
    "does",
    "did",
    "doing",
    "have",
    "has",
    "had",
    "will",
    "would",
    "can",
    "could",
    "should",
    "shall",
    "may",
    "might",
    "must",
    "my",
    "me",
    "us",
    "of",
    "in",
    "on",
    "at",
    "by",
    "for",
    "with",
    "from",
    "to",
    "as",
    "into",
    "about",
    "um",
    "uh",
    "erm",
    "ah",
    "er",
    "oh",
    "like",
    "just",
    "really",
    "very",
    "okay",
    "ok",
    "yes",
    "no",
    "yeah",
    "yep",
    "nope",
    "actually",
    "basically",
    "literally",
    "probably",
    "maybe",
    "kinda",
    "sorta",
    "new",
    "line",
    "paragraph",
    "bullet",
    "bullets",
    "numbered",
    "list",
    "heading",
    "title",
    "period",
    "comma",
    "exclamation",
    "question",
    "mark",
];

pub fn is_enabled(db: &Store) -> bool {
    db.get_setting("autoLearnVocabulary")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str::<bool>(&v).ok())
        .unwrap_or(true)
}

/// Entry point called by the pipeline after a successful cleanup pass.
pub fn observe(db: &Store, raw_text: &str, polished: &str) {
    if !is_enabled(db) {
        return;
    }
    if let Err(e) = try_observe(db, raw_text, polished) {
        eprintln!("vocab learning skipped: {e}");
    }
}

fn try_observe(db: &Store, raw_text: &str, polished: &str) -> Result<()> {
    let raw = tokenize(raw_text);
    let pol = tokenize(polished);
    if raw.is_empty()
        || pol.is_empty()
        || raw.len() > MAX_TOKENS_FOR_DIFF
        || pol.len() > MAX_TOKENS_FOR_DIFF
    {
        return Ok(());
    }

    let first_pol_key = pol[0].key.clone();
    let ops = diff_ops(&raw, &pol);
    let mut candidates = extract_candidates(&raw, &pol, &ops, &first_pol_key);

    // Strong signals first so the session cap favors silent adds over queue
    // entries; within a tier discovery order is preserved.
    candidates.sort_by_key(|c| std::cmp::Reverse(c.silent));

    let mut recorded = 0;
    let mut seen = HashSet::new();
    for cand in candidates {
        if recorded >= MAX_CANDIDATES_PER_SESSION {
            break;
        }
        if !seen.insert(cand.term_key.clone()) {
            continue;
        }
        if db.dictionary_contains(&cand.term)? {
            continue;
        }
        if cand.silent {
            db.auto_learn_term(&cand.term)?;
            eprintln!("learned vocabulary: {:?}", cand.term);
        } else {
            db.record_vocab_suggestion(&cand.raw_form, &cand.term)?;
        }
        recorded += 1;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
struct Token {
    /// Lowercased alphanumeric key used for matching.
    key: String,
    /// Original surface form.
    orig: String,
}

fn tokenize(text: &str) -> Vec<Token> {
    text.split_whitespace()
        .map(|w| Token {
            key: w
                .chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(|c| c.to_lowercase())
                .collect(),
            orig: surface(w),
        })
        .filter(|t| !t.key.is_empty())
        .collect()
}

/// Trims edge punctuation from a surface form while keeping interior marks
/// (apostrophes, hyphens, dots inside version numbers).
fn surface(orig: &str) -> String {
    orig.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
        .to_string()
}

#[derive(Debug, Clone, Copy)]
enum Op {
    Match(usize, usize),
    DelRaw(usize),
    AddPol(usize),
}

/// Word-level LCS diff between the raw and polished token streams.
fn diff_ops(raw: &[Token], pol: &[Token]) -> Vec<Op> {
    let n = raw.len();
    let m = pol.len();
    // u16 suffices: streams are capped at MAX_TOKENS_FOR_DIFF.
    let mut table = vec![vec![0u16; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            table[i][j] = if raw[i - 1].key == pol[j - 1].key {
                table[i - 1][j - 1] + 1
            } else {
                table[i - 1][j].max(table[i][j - 1])
            };
        }
    }

    let mut ops = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 && j > 0 {
        if raw[i - 1].key == pol[j - 1].key {
            ops.push(Op::Match(i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if table[i - 1][j] >= table[i][j - 1] {
            ops.push(Op::DelRaw(i - 1));
            i -= 1;
        } else {
            ops.push(Op::AddPol(j - 1));
            j -= 1;
        }
    }
    while i > 0 {
        ops.push(Op::DelRaw(i - 1));
        i -= 1;
    }
    while j > 0 {
        ops.push(Op::AddPol(j - 1));
        j -= 1;
    }
    ops.reverse();
    ops
}

#[derive(Debug, Clone)]
struct Candidate {
    term: String,
    term_key: String,
    raw_form: String,
    /// true = add straight to the dictionary, false = review queue.
    silent: bool,
}

/// Walks the diff ops pairing deleted/added runs into replacement segments,
/// plus case-fix matches (same key, different casing).
fn extract_candidates(
    raw: &[Token],
    pol: &[Token],
    ops: &[Op],
    first_pol_key: &str,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut dels: Vec<Token> = Vec::new();
    let mut adds: Vec<Token> = Vec::new();

    let flush = |dels: &mut Vec<Token>, adds: &mut Vec<Token>, out: &mut Vec<Candidate>| {
        if dels.is_empty() || adds.is_empty() {
            // Pure filler deletion or insertion carries no vocabulary signal.
        } else if let Some(cand) = eval_replacement(dels, adds, first_pol_key) {
            out.push(cand);
        }
        dels.clear();
        adds.clear();
    };

    for op in ops {
        match *op {
            Op::DelRaw(i) => dels.push(raw[i].clone()),
            Op::AddPol(j) => adds.push(pol[j].clone()),
            Op::Match(i, j) => {
                if raw[i].orig == pol[j].orig {
                    // A genuinely identical word ends any open run.
                    flush(&mut dels, &mut adds, &mut out);
                } else {
                    // Same word, different casing (or near-identical
                    // spelling): fold into the open del/add run so
                    // multi-word corrections stay one segment — "jon
                    // smyth" → "Jon Smythe" is learned as the full name,
                    // not split into an orphan case-fix plus a surname.
                    dels.push(raw[i].clone());
                    adds.push(pol[j].clone());
                }
            }
        }
    }
    flush(&mut dels, &mut adds, &mut out);
    out
}

fn starts_upper(tok: &Token) -> bool {
    tok.orig.starts_with(|c: char| c.is_uppercase())
}

fn learnable_word(tok: &Token) -> bool {
    tok.key.len() >= 2
        && tok.key.chars().count() <= MAX_TERM_CHARS
        && !STOPWORDS.contains(&tok.key.as_str())
}

fn eval_replacement(dels: &[Token], adds: &[Token], first_pol_key: &str) -> Option<Candidate> {
    if dels.len() > MAX_SEGMENT_TOKENS || adds.len() > MAX_SEGMENT_TOKENS {
        return None;
    }
    if !adds.iter().all(learnable_word) {
        return None;
    }

    let joined_raw: String = dels.iter().map(|t| t.key.as_str()).collect();
    let joined_pol: String = adds.iter().map(|t| t.key.as_str()).collect();
    if joined_raw == joined_pol {
        // Same words, different casing only ("kubernetes" → "Kubernetes").
        // Capitalization alone is weak evidence, so these go to the review
        // queue instead of the dictionary — except sentence-initial words,
        // which would spam the queue.
        let any_change = dels.iter().zip(adds.iter()).any(|(r, p)| r.orig != p.orig);
        if !any_change || (adds.len() == 1 && adds[0].key == first_pol_key) {
            return None;
        }
        return Some(Candidate {
            term: adds
                .iter()
                .map(|t| t.orig.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            term_key: joined_pol,
            raw_form: dels
                .iter()
                .map(|t| t.orig.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            silent: false,
        });
    }
    let ratio = similarity_ratio(&joined_raw, &joined_pol);
    if ratio > MAX_SIMILARITY_RATIO {
        return None;
    }

    // Capitalization is only a proper-noun signal away from sentence starts:
    // the global-first token of the polished text is discounted.
    let capitalized = adds
        .iter()
        .filter(|t| t.key != first_pol_key)
        .filter(|t| starts_upper(t))
        .count();

    let silent = capitalized == adds.len() && capitalized > 0;
    let queued = adds.len() == 1 && ratio <= TIGHT_SIMILARITY_RATIO || capitalized > 0;
    if !silent && !queued {
        return None;
    }

    Some(Candidate {
        term: adds
            .iter()
            .map(|t| t.orig.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        term_key: joined_pol,
        raw_form: dels
            .iter()
            .map(|t| t.orig.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        silent,
    })
}

fn similarity_ratio(a: &str, b: &str) -> f64 {
    let max = a.chars().count().max(b.chars().count());
    if max == 0 {
        return 0.0;
    }
    levenshtein(a, b) as f64 / max as f64
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            cur[j] = if a[i - 1] == b[j - 1] {
                prev[j - 1]
            } else {
                1 + prev[j - 1].min(prev[j]).min(cur[j - 1])
            };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn store() -> Store {
        Store::open(std::path::Path::new(":memory:")).unwrap()
    }

    #[test]
    fn proper_noun_fix_is_learned_silently() {
        let db = store();
        observe(
            &db,
            "met with jon smyth about akme today",
            "Met with Jon Smythe about Acme today.",
        );
        assert!(db.dictionary_contains("Jon Smythe").unwrap());
        assert!(db.dictionary_contains("Acme").unwrap());
        assert!(db.list_vocab_suggestions().unwrap().is_empty());
    }

    #[test]
    fn filler_removal_is_ignored() {
        let db = store();
        observe(
            &db,
            "um so hello world this is a test",
            "Hello world, this is a test.",
        );
        assert!(db.list_dictionary().unwrap().is_empty());
        assert!(db.list_vocab_suggestions().unwrap().is_empty());
    }

    #[test]
    fn rephrasing_is_ignored() {
        let db = store();
        observe(
            &db,
            "I think maybe we should go over there quickly ok",
            "We should head there soon.",
        );
        assert!(db.list_dictionary().unwrap().is_empty());
        assert!(db.list_vocab_suggestions().unwrap().is_empty());
    }

    #[test]
    fn case_only_fix_goes_to_review_queue() {
        let db = store();
        observe(
            &db,
            "please email the kubernetes team",
            "Please email the Kubernetes team.",
        );
        assert!(!db.dictionary_contains("Kubernetes").unwrap());
        let suggestions = db.list_vocab_suggestions().unwrap();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].term, "Kubernetes");
    }

    #[test]
    fn sentence_start_and_pronoun_case_fixes_are_skipped() {
        let db = store();
        observe(&db, "i think it is done", "I think it is done.");
        assert!(db.list_vocab_suggestions().unwrap().is_empty());
    }

    #[test]
    fn known_terms_are_not_relearned() {
        let db = store();
        db.add_dictionary_term("Jon Smythe", None).unwrap();
        observe(
            &db,
            "met with jon smyth yesterday",
            "Met with Jon Smythe yesterday.",
        );
        assert!(db.list_vocab_suggestions().unwrap().is_empty());
    }

    #[test]
    fn session_cap_limits_records() {
        let db = store();
        observe(
            &db,
            "call alpa about betta then gamna near deta over epslon now",
            "Call Alpha about Beta then Gamma near Delta over Epsilon now.",
        );
        // Five distinct proper-noun fixes, only three recorded.
        assert_eq!(db.list_dictionary().unwrap().len(), 3);
        assert!(db.list_vocab_suggestions().unwrap().is_empty());
    }

    #[test]
    fn disabled_setting_skips_learning() {
        let db = store();
        db.set_setting("autoLearnVocabulary", &serde_json::json!(false))
            .unwrap();
        assert!(!is_enabled(&db));
        observe(
            &db,
            "met with john smyth yesterday",
            "Met with John Smythe yesterday.",
        );
        assert!(db.list_dictionary().unwrap().is_empty());
    }

    #[test]
    fn mangled_single_token_jargon_is_queued() {
        let db = store();
        observe(
            &db,
            "open the rust anlyzer docs",
            "Open the rust-analyzer docs.",
        );
        let suggestions = db.list_vocab_suggestions().unwrap();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].term, "rust-analyzer");
        assert!(!db.dictionary_contains("rust-analyzer").unwrap());
    }

    #[test]
    fn similarity_ratio_basics() {
        assert_eq!(similarity_ratio("", ""), 0.0);
        assert!(similarity_ratio("johnsmyth", "johnsmythe") < 0.2);
        assert!(similarity_ratio("asapplease", "immediately") > MAX_SIMILARITY_RATIO);
    }
}
