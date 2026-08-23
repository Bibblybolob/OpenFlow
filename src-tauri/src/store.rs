use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl Serialize for StoreError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub id: i64,
    pub text: String,
    pub raw_text: String,
    pub language: String,
    pub duration_ms: i64,
    pub word_count: i64,
    pub target_app: String,
    pub flagged: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DictionaryEntry {
    pub id: i64,
    pub term: String,
    pub replacement: Option<String>,
    pub starred: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub id: i64,
    pub trigger: String,
    pub body: String,
    pub created_at: String,
}

/// A vocabulary candidate detected automatically from dictation diffs.
/// Pending rows surface in the Dictionary page for one-click review;
/// accepted/dismissed rows are kept to avoid re-suggesting the same term.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VocabSuggestion {
    pub id: i64,
    pub raw_form: String,
    pub term: String,
    pub occurrences: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Style {
    pub id: i64,
    pub app_pattern: String,
    pub label: String,
    pub instructions: String,
    pub enabled: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub total_words: i64,
    pub transcript_count: i64,
    pub streak_days: i64,
}

pub struct Store {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS transcripts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    text        TEXT NOT NULL,
    raw_text    TEXT NOT NULL DEFAULT '',
    language    TEXT NOT NULL DEFAULT 'auto',
    duration_ms INTEGER NOT NULL DEFAULT 0,
    word_count  INTEGER NOT NULL DEFAULT 0,
    target_app  TEXT NOT NULL DEFAULT '',
    flagged     INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_transcripts_created ON transcripts (created_at DESC);

CREATE TABLE IF NOT EXISTS dictionary (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    term        TEXT NOT NULL COLLATE NOCASE UNIQUE,
    replacement TEXT,
    starred     INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS snippets (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    trigger    TEXT NOT NULL COLLATE NOCASE UNIQUE,
    body       TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS vocab_suggestions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    raw_form    TEXT NOT NULL,
    term        TEXT NOT NULL COLLATE NOCASE UNIQUE,
    occurrences INTEGER NOT NULL DEFAULT 1,
    status      TEXT NOT NULL DEFAULT 'pending',
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS styles (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    app_pattern  TEXT NOT NULL UNIQUE COLLATE NOCASE,
    label        TEXT NOT NULL,
    instructions TEXT NOT NULL,
    enabled      INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn insert_transcript(
        &self,
        text: &str,
        raw_text: &str,
        language: &str,
        duration_ms: i64,
        target_app: &str,
    ) -> Result<Transcript> {
        let word_count = text.split_whitespace().count() as i64;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO transcripts (text, raw_text, language, duration_ms, word_count, target_app)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![text, raw_text, language, duration_ms, word_count, target_app],
        )?;
        Ok(Transcript {
            id: conn.last_insert_rowid(),
            text: text.to_string(),
            raw_text: raw_text.to_string(),
            language: language.to_string(),
            duration_ms,
            word_count,
            target_app: target_app.to_string(),
            flagged: false,
            created_at: now_iso(&conn)?,
        })
    }

    pub fn list_transcripts(&self, limit: i64, offset: i64) -> Result<Vec<Transcript>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, text, raw_text, language, duration_ms, word_count, target_app, flagged, created_at
             FROM transcripts ORDER BY created_at DESC, id DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit, offset], map_transcript)?;
        rows.map(|r| r.map_err(StoreError::from)).collect()
    }

    pub fn search_transcripts(&self, query: &str) -> Result<Vec<Transcript>> {
        let pattern = format!("%{}%", query);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, text, raw_text, language, duration_ms, word_count, target_app, flagged, created_at
             FROM transcripts WHERE text LIKE ?1 OR raw_text LIKE ?1
             ORDER BY created_at DESC, id DESC LIMIT 200",
        )?;
        let rows = stmt.query_map(params![pattern], map_transcript)?;
        rows.map(|r| r.map_err(StoreError::from)).collect()
    }

    pub fn delete_transcript(&self, id: i64) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM transcripts WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn set_flagged(&self, id: i64, flagged: bool) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE transcripts SET flagged = ?2 WHERE id = ?1",
            params![id, flagged],
        )?;
        Ok(())
    }

    pub fn stats(&self) -> Result<Stats> {
        let conn = self.conn.lock().unwrap();
        let total_words: i64 = conn.query_row(
            "SELECT COALESCE(SUM(word_count), 0) FROM transcripts",
            [],
            |r| r.get(0),
        )?;
        let transcript_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM transcripts", [], |r| r.get(0))?;

        let mut stmt = conn.prepare(
            "SELECT DISTINCT date(created_at) FROM transcripts ORDER BY date(created_at) DESC LIMIT 400",
        )?;
        let dates: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let streak = if dates.is_empty() {
            0
        } else {
            let today = local_date_today();
            let first_gap = days_between(&dates[0], &today);
            if first_gap > 1 {
                0
            } else {
                let mut streak: i64 = 1;
                for pair in dates.windows(2) {
                    if days_between(&pair[1], &pair[0]) == 1 {
                        streak += 1;
                    } else {
                        break;
                    }
                }
                streak
            }
        };

        Ok(Stats {
            total_words,
            transcript_count,
            streak_days: streak,
        })
    }

    pub fn add_dictionary_term(
        &self,
        term: &str,
        replacement: Option<&str>,
    ) -> Result<DictionaryEntry> {
        let starred = 0;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO dictionary (term, replacement, starred) VALUES (?1, ?2, ?3)
             ON CONFLICT(term) DO UPDATE SET replacement = excluded.replacement",
            params![term.trim(), replacement, starred],
        )?;
        let id = conn.last_insert_rowid();
        Ok(DictionaryEntry {
            id,
            term: term.trim().to_string(),
            replacement: replacement.map(str::to_string),
            starred: false,
            created_at: now_iso(&conn)?,
        })
    }

    pub fn list_dictionary(&self) -> Result<Vec<DictionaryEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, term, replacement, starred, created_at
             FROM dictionary ORDER BY starred DESC, term ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(DictionaryEntry {
                id: row.get(0)?,
                term: row.get(1)?,
                replacement: row.get(2)?,
                starred: row.get::<_, i64>(3)? != 0,
                created_at: row.get(4)?,
            })
        })?;
        rows.map(|r| r.map_err(StoreError::from)).collect()
    }

    pub fn set_dictionary_starred(&self, id: i64, starred: bool) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE dictionary SET starred = ?2 WHERE id = ?1",
            params![id, starred],
        )?;
        Ok(())
    }

    pub fn delete_dictionary_term(&self, id: i64) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM dictionary WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Case-insensitive check whether a term is already in the dictionary.
    pub fn dictionary_contains(&self, term: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let hit = conn
            .query_row(
                "SELECT 1 FROM dictionary WHERE term = ?1 COLLATE NOCASE",
                params![term.trim()],
                |r| r.get::<_, i64>(0),
            )
            .optional()?;
        Ok(hit.is_some())
    }

    /// Silently adds a learned term to the dictionary. Existing entries are
    /// left untouched so manual replacements are never overwritten.
    pub fn auto_learn_term(&self, term: &str) -> Result<()> {
        let term = term.trim();
        if term.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO dictionary (term) VALUES (?1)
             ON CONFLICT(term) DO NOTHING",
            params![term],
        )?;
        Ok(())
    }

    /// Records a low-confidence vocabulary candidate. Repeat sightings bump
    /// the occurrence counter; dismissed suggestions stay dismissed.
    pub fn record_vocab_suggestion(&self, raw_form: &str, term: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO vocab_suggestions (raw_form, term) VALUES (?1, ?2)
             ON CONFLICT(term) DO UPDATE SET occurrences = occurrences + 1",
            params![raw_form.trim(), term.trim()],
        )?;
        Ok(())
    }

    pub fn list_vocab_suggestions(&self) -> Result<Vec<VocabSuggestion>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, raw_form, term, occurrences, created_at
             FROM vocab_suggestions WHERE status = 'pending'
             ORDER BY occurrences DESC, id DESC LIMIT 200",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(VocabSuggestion {
                id: row.get(0)?,
                raw_form: row.get(1)?,
                term: row.get(2)?,
                occurrences: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.map(|r| r.map_err(StoreError::from)).collect()
    }

    /// Promotes a suggestion into the dictionary and marks it accepted.
    pub fn accept_vocab_suggestion(&self, id: i64) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let term: Option<String> = conn
            .query_row(
                "SELECT term FROM vocab_suggestions WHERE id = ?1 AND status = 'pending'",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(term) = term else {
            return Ok(());
        };
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO dictionary (term) VALUES (?1)
             ON CONFLICT(term) DO NOTHING",
            params![term],
        )?;
        tx.execute(
            "UPDATE vocab_suggestions SET status = 'accepted' WHERE id = ?1",
            params![id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn dismiss_vocab_suggestion(&self, id: i64) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE vocab_suggestions SET status = 'dismissed'
             WHERE id = ?1 AND status = 'pending'",
            params![id],
        )?;
        Ok(())
    }

    pub fn add_snippet(&self, trigger: &str, body: &str) -> Result<Snippet> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO snippets (trigger, body) VALUES (?1, ?2)
             ON CONFLICT(trigger) DO UPDATE SET body = excluded.body",
            params![trigger.trim(), body],
        )?;
        let id = conn.last_insert_rowid();
        Ok(Snippet {
            id,
            trigger: trigger.trim().to_string(),
            body: body.to_string(),
            created_at: now_iso(&conn)?,
        })
    }

    pub fn list_snippets(&self) -> Result<Vec<Snippet>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, trigger, body, created_at FROM snippets ORDER BY trigger ASC")?;
        let rows = stmt.query_map([], |row| {
            Ok(Snippet {
                id: row.get(0)?,
                trigger: row.get(1)?,
                body: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.map(|r| r.map_err(StoreError::from)).collect()
    }

    pub fn delete_snippet(&self, id: i64) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM snippets WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn upsert_style(
        &self,
        app_pattern: &str,
        label: &str,
        instructions: &str,
    ) -> Result<Style> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO styles (app_pattern, label, instructions) VALUES (?1, ?2, ?3)
             ON CONFLICT(app_pattern) DO UPDATE SET label = excluded.label, instructions = excluded.instructions",
            params![app_pattern.trim().to_lowercase(), label, instructions],
        )?;
        let id = conn.last_insert_rowid();
        Ok(Style {
            id,
            app_pattern: app_pattern.trim().to_lowercase(),
            label: label.to_string(),
            instructions: instructions.to_string(),
            enabled: true,
            created_at: now_iso(&conn)?,
        })
    }

    pub fn list_styles(&self) -> Result<Vec<Style>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, app_pattern, label, instructions, enabled, created_at
             FROM styles ORDER BY app_pattern ASC",
        )?;
        let rows = stmt.query_map([], map_style)?;
        rows.map(|r| r.map_err(StoreError::from)).collect()
    }

    /// Resolves the style instructions for a frontmost app. Among all enabled
    /// patterns contained in the identifier, the most specific (longest) wins,
    /// e.g. "com.apple.mail" beats "mail".
    pub fn resolve_style_for_app(&self, app_identifier: &str) -> Result<Option<String>> {
        let needle = app_identifier.to_lowercase();
        let all = self.list_styles()?;
        Ok(all
            .iter()
            .filter(|s| s.enabled && !s.app_pattern.is_empty() && needle.contains(&s.app_pattern))
            .max_by_key(|s| s.app_pattern.len())
            .map(|s| s.instructions.clone()))
    }

    pub fn set_style_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE styles SET enabled = ?2 WHERE id = ?1",
            params![id, enabled],
        )?;
        Ok(())
    }

    pub fn delete_style(&self, id: i64) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM styles WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        let encoded = serde_json::to_string(value)?;
        self.conn.lock().unwrap().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, encoded],
        )?;
        Ok(())
    }
}

fn map_transcript(row: &rusqlite::Row<'_>) -> rusqlite::Result<Transcript> {
    Ok(Transcript {
        id: row.get(0)?,
        text: row.get(1)?,
        raw_text: row.get(2)?,
        language: row.get(3)?,
        duration_ms: row.get(4)?,
        word_count: row.get(5)?,
        target_app: row.get(6)?,
        flagged: row.get::<_, i64>(7)? != 0,
        created_at: row.get(8)?,
    })
}

fn map_style(row: &rusqlite::Row<'_>) -> rusqlite::Result<Style> {
    Ok(Style {
        id: row.get(0)?,
        app_pattern: row.get(1)?,
        label: row.get(2)?,
        instructions: row.get(3)?,
        enabled: row.get::<_, i64>(4)? != 0,
        created_at: row.get(5)?,
    })
}

fn now_iso(conn: &Connection) -> Result<String> {
    Ok(
        conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |r| {
            r.get(0)
        })?,
    )
}

fn local_date_today() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = now / 86_400;
    civil_from_days(days)
}

fn days_between(from_iso_date: &str, to_iso_date: &str) -> i64 {
    let parse = |s: &str| -> i64 {
        let parts: Vec<i64> = s.split('-').filter_map(|p| p.parse().ok()).collect();
        if parts.len() < 3 {
            return i64::MAX / 2;
        }
        days_from_civil(parts[0], parts[1], parts[2])
    };
    parse(to_iso_date) - parse(from_iso_date)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> String {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_store() -> Store {
        Store::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn schema_applies_and_stats_start_empty() {
        let store = memory_store();
        let stats = store.stats().unwrap();
        assert_eq!(stats.total_words, 0);
        assert_eq!(stats.transcript_count, 0);
        assert_eq!(stats.streak_days, 0);
    }

    #[test]
    fn transcript_roundtrip_and_word_count() {
        let store = memory_store();
        store
            .insert_transcript("hello world foo", "um hello world foo", "en", 1200, "Slack")
            .unwrap();
        let list = store.list_transcripts(10, 0).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].word_count, 3);
        assert_eq!(list[0].target_app, "Slack");
        assert!(!list[0].flagged);
    }

    #[test]
    fn delete_and_flag_transcripts() {
        let store = memory_store();
        let t = store.insert_transcript("a b c", "", "en", 500, "").unwrap();
        store.set_flagged(t.id, true).unwrap();
        assert!(store.list_transcripts(10, 0).unwrap()[0].flagged);
        store.delete_transcript(t.id).unwrap();
        assert!(store.list_transcripts(10, 0).unwrap().is_empty());
    }

    #[test]
    fn dictionary_upsert_on_conflict() {
        let store = memory_store();
        store.add_dictionary_term("kubernetes", None).unwrap();
        store
            .add_dictionary_term("kubernetes", Some("Kubernetes"))
            .unwrap();
        let list = store.list_dictionary().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].replacement.as_deref(), Some("Kubernetes"));
    }

    #[test]
    fn snippet_upsert_on_conflict() {
        let store = memory_store();
        store.add_snippet("my email", "jon@example.com").unwrap();
        store
            .add_snippet("my email", "jonathan@example.com")
            .unwrap();
        let list = store.list_snippets().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].body, "jonathan@example.com");
    }

    #[test]
    fn style_resolves_by_substring_case_insensitive() {
        let store = memory_store();
        store
            .upsert_style("com.apple.mail", "Email formal", "Formal tone")
            .unwrap();
        assert_eq!(
            store
                .resolve_style_for_app("COM.APPLE.MAIL")
                .unwrap()
                .as_deref(),
            Some("Formal tone")
        );
        assert_eq!(
            store.resolve_style_for_app("com.slack.Slack").unwrap(),
            None
        );
    }

    #[test]
    fn style_most_specific_pattern_wins() {
        let store = memory_store();
        store
            .upsert_style("mail", "Mail generic", "Generic tone")
            .unwrap();
        store
            .upsert_style("com.apple.mail", "Apple Mail", "Apple-specific tone")
            .unwrap();
        assert_eq!(
            store
                .resolve_style_for_app("com.apple.mail")
                .unwrap()
                .as_deref(),
            Some("Apple-specific tone")
        );
        // Substring semantics: the pattern "mail" does not occur inside
        // Thunderbird's identifier, so nothing matches there.
        assert_eq!(
            store
                .resolve_style_for_app("org.mozilla.thunderbird")
                .unwrap(),
            None
        );
    }

    #[test]
    fn vocab_suggestion_upsert_counts_occurrences() {
        let store = memory_store();
        store
            .record_vocab_suggestion("john smyth", "John Smythe")
            .unwrap();
        store
            .record_vocab_suggestion("jon smythe", "John Smythe")
            .unwrap();
        let list = store.list_vocab_suggestions().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].term, "John Smythe");
        assert_eq!(list[0].occurrences, 2);
    }

    #[test]
    fn accept_moves_suggestion_into_dictionary() {
        let store = memory_store();
        store
            .record_vocab_suggestion("kubernets", "Kubernetes")
            .unwrap();
        let id = store.list_vocab_suggestions().unwrap()[0].id;
        store.accept_vocab_suggestion(id).unwrap();
        assert!(store.dictionary_contains("kubernetes").unwrap());
        assert!(store.list_vocab_suggestions().unwrap().is_empty());

        // Accepted terms are never re-suggested.
        store
            .record_vocab_suggestion("kubernets", "Kubernetes")
            .unwrap();
        assert!(store.list_vocab_suggestions().unwrap().is_empty());
    }

    #[test]
    fn dismiss_keeps_term_dismissed() {
        let store = memory_store();
        store.record_vocab_suggestion("acme", "ACME").unwrap();
        let id = store.list_vocab_suggestions().unwrap()[0].id;
        store.dismiss_vocab_suggestion(id).unwrap();
        assert!(!store.dictionary_contains("ACME").unwrap());
        // Repeated sightings must not resurrect a dismissed suggestion.
        store.record_vocab_suggestion("acme", "ACME").unwrap();
        assert!(store.list_vocab_suggestions().unwrap().is_empty());
    }

    #[test]
    fn auto_learn_is_idempotent_and_preserves_manual_entries() {
        let store = memory_store();
        store
            .add_dictionary_term("smythe", Some("Smythe & Co"))
            .unwrap();
        store.auto_learn_term("SMYTHE").unwrap();
        store.auto_learn_term("").unwrap();
        let list = store.list_dictionary().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].replacement.as_deref(), Some("Smythe & Co"));
        store.auto_learn_term("Brand New Term").unwrap();
        assert!(store.dictionary_contains("brand new term").unwrap());
    }

    #[test]
    fn settings_roundtrip() {
        let store = memory_store();
        assert_eq!(store.get_setting("hotkey").unwrap(), None);
        store
            .set_setting("hotkey", &serde_json::json!({"keys": ["Fn"]}))
            .unwrap();
        assert_eq!(
            store.get_setting("hotkey").unwrap().as_deref(),
            Some("{\"keys\":[\"Fn\"]}")
        );
    }

    #[test]
    fn streak_counts_consecutive_days() {
        let store = memory_store();
        store
            .insert_transcript("one two three four five", "", "en", 1000, "")
            .unwrap();
        let stats = store.stats().unwrap();
        assert_eq!(stats.total_words, 5);
        assert_eq!(stats.streak_days, 1);
    }
}
