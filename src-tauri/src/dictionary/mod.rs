//! The dictionary: names, abbreviations and terms the speech model and the
//! post-processing LLM should spell right.
//!
//! Words come from the user (manual), from corrections the user makes to a
//! pasted dictation (learned, see `edit_monitor`), and from the LLM naming
//! the uncommon terms it saw in a dictation or a meeting (suggested). Each
//! entry is active, pending review, or dismissed; dismissed entries stay so
//! the same term is not proposed again.
//!
//! The active words are mirrored into `AppSettings::custom_words`, which is
//! what the transcription pipeline already reads (as model vocabulary or for
//! fuzzy correction), so no transcription code needs to know about the
//! store. The mirror is ordered by priority (manual words first, then the
//! most recently used), so when a model only reads the head of its
//! vocabulary prompt, the words that matter most survive.

pub mod edit_monitor;
pub mod learner;
#[cfg(target_os = "windows")]
mod windows;

use anyhow::{anyhow, Result};
use log::{debug, info, warn};
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use tauri_specta::Event;

use crate::settings::{get_settings, write_settings};
use learner::{normalize_word, word_key};

static MIGRATIONS: &[M] = &[M::up(
    "CREATE TABLE IF NOT EXISTS dictionary (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        word TEXT NOT NULL,
        word_key TEXT NOT NULL UNIQUE,
        source TEXT NOT NULL,
        status TEXT NOT NULL,
        origin TEXT,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        last_used_at INTEGER,
        use_count INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX IF NOT EXISTS idx_dictionary_status ON dictionary(status);",
)];

/// How many terms one dictation or meeting may propose at once.
pub const MAX_TERMS_PER_SUGGESTION: usize = 12;

/// Where an entry came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum DictionarySource {
    /// Typed in by the user.
    Manual,
    /// Picked up from the user correcting a pasted dictation.
    Learned,
    /// Proposed by the LLM from a dictation or a meeting.
    Suggested,
}

impl DictionarySource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Learned => "learned",
            Self::Suggested => "suggested",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "learned" => Self::Learned,
            "suggested" => Self::Suggested,
            _ => Self::Manual,
        }
    }
}

/// Whether an entry takes part in transcription.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum DictionaryStatus {
    /// Given to the speech model and the LLM.
    Active,
    /// Waiting for the user to accept or dismiss it.
    Pending,
    /// Rejected by the user; never proposed again.
    Dismissed,
}

impl DictionaryStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Pending => "pending",
            Self::Dismissed => "dismissed",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "pending" => Self::Pending,
            "dismissed" => Self::Dismissed,
            _ => Self::Active,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct DictionaryEntry {
    pub id: i64,
    pub word: String,
    pub source: DictionarySource,
    pub status: DictionaryStatus,
    /// Where a learned or suggested word was found, for the review list.
    pub origin: Option<String>,
    /// Unix milliseconds.
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used_at: Option<i64>,
    /// How many dictations the word has appeared in since it was added.
    pub use_count: i64,
}

/// Fired after any change to the dictionary. `added` names the words that
/// were just learned or suggested, if any, so the UI can mention them.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct DictionaryUpdatedEvent {
    pub added: Vec<String>,
    pub pending: u32,
    pub active: u32,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub struct DictionaryManager {
    app: AppHandle,
    db_path: PathBuf,
    /// Serializes writers; SQLite handles readers.
    write_lock: Mutex<()>,
}

impl DictionaryManager {
    pub fn new(app: &AppHandle) -> Result<Self> {
        let app_data_dir = crate::portable::app_data_dir(app)?;
        let manager = Self {
            app: app.clone(),
            db_path: app_data_dir.join("dictionary.db"),
            write_lock: Mutex::new(()),
        };
        manager.init_database()?;
        manager.import_custom_words_once()?;
        manager.sync_settings(Vec::new());
        Ok(manager)
    }

    fn init_database(&self) -> Result<()> {
        info!("Initializing dictionary database at {:?}", self.db_path);
        let mut conn = Connection::open(&self.db_path)?;
        let migrations = Migrations::new(MIGRATIONS.to_vec());
        #[cfg(debug_assertions)]
        migrations
            .validate()
            .expect("Invalid dictionary migrations");
        migrations.to_latest(&mut conn)?;
        Ok(())
    }

    fn connection(&self) -> Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    /// The words of the old flat `custom_words` setting become manual
    /// entries the first time the store is empty.
    fn import_custom_words_once(&self) -> Result<()> {
        let conn = self.connection()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM dictionary", [], |row| row.get(0))?;
        if count > 0 {
            return Ok(());
        }
        let words = get_settings(&self.app).custom_words;
        if words.is_empty() {
            return Ok(());
        }
        let added = self.insert_words(
            &conn,
            &words,
            DictionarySource::Manual,
            DictionaryStatus::Active,
            None,
        )?;
        info!(
            "Imported {} custom word(s) into the dictionary",
            added.len()
        );
        Ok(())
    }

    fn map_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<DictionaryEntry> {
        let source: String = row.get("source")?;
        let status: String = row.get("status")?;
        Ok(DictionaryEntry {
            id: row.get("id")?,
            word: row.get("word")?,
            source: DictionarySource::parse(&source),
            status: DictionaryStatus::parse(&status),
            origin: row.get("origin")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            last_used_at: row.get("last_used_at")?,
            use_count: row.get("use_count")?,
        })
    }

    /// Every entry, newest first.
    pub fn list(&self) -> Result<Vec<DictionaryEntry>> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, word, source, status, origin, created_at, updated_at, last_used_at, use_count
             FROM dictionary ORDER BY created_at DESC, id DESC",
        )?;
        let entries = stmt
            .query_map([], Self::map_entry)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(entries)
    }

    /// The words transcription should know, most important first: manual
    /// words, then by most recent use, then the newest.
    pub fn active_words(&self) -> Result<Vec<String>> {
        let conn = self.connection()?;
        Self::active_words_in(&conn)
    }

    fn active_words_in(conn: &Connection) -> Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT word FROM dictionary WHERE status = 'active'
             ORDER BY (source = 'manual') DESC, COALESCE(last_used_at, 0) DESC, use_count DESC, created_at DESC",
        )?;
        let words = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(words)
    }

    fn counts(conn: &Connection) -> Result<(u32, u32)> {
        let pending: u32 = conn.query_row(
            "SELECT COUNT(*) FROM dictionary WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )?;
        let active: u32 = conn.query_row(
            "SELECT COUNT(*) FROM dictionary WHERE status = 'active'",
            [],
            |row| row.get(0),
        )?;
        Ok((pending, active))
    }

    /// Inserts the words not in the store yet (by case-insensitive key) and
    /// returns them as stored.
    fn insert_words(
        &self,
        conn: &Connection,
        words: &[String],
        source: DictionarySource,
        status: DictionaryStatus,
        origin: Option<&str>,
    ) -> Result<Vec<String>> {
        let now = now_ms();
        let mut added = Vec::new();
        for word in words {
            let Some(word) = normalize_word(word) else {
                continue;
            };
            let key = word_key(&word);
            let inserted = conn.execute(
                "INSERT OR IGNORE INTO dictionary (word, word_key, source, status, origin, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![word, key, source.as_str(), status.as_str(), origin, now],
            )?;
            if inserted > 0 {
                added.push(word);
            }
        }
        Ok(added)
    }

    /// Adds words the user typed. A word that was pending or dismissed
    /// becomes an active manual word.
    pub fn add_manual(&self, words: &[String]) -> Result<Vec<String>> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.connection()?;
        let mut added = self.insert_words(
            &conn,
            words,
            DictionarySource::Manual,
            DictionaryStatus::Active,
            None,
        )?;
        let now = now_ms();
        for word in words {
            let Some(word) = normalize_word(word) else {
                continue;
            };
            let promoted = conn.execute(
                "UPDATE dictionary SET word = ?1, source = 'manual', status = 'active', updated_at = ?2
                 WHERE word_key = ?3 AND status != 'active'",
                params![word, now, word_key(&word)],
            )?;
            if promoted > 0 {
                added.push(word);
            }
        }
        drop(conn);
        self.sync_settings(Vec::new());
        Ok(added)
    }

    /// Proposes words found by a learner. They become active right away or
    /// wait for review, per the `dictionary_auto_accept` setting. Words the
    /// store already knows (active, pending or dismissed) are skipped.
    pub fn suggest(
        &self,
        words: &[String],
        source: DictionarySource,
        origin: &str,
    ) -> Result<Vec<String>> {
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let status = if get_settings(&self.app).dictionary_auto_accept {
            DictionaryStatus::Active
        } else {
            DictionaryStatus::Pending
        };
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.connection()?;
        let added = self.insert_words(&conn, words, source, status, Some(origin))?;
        drop(conn);
        if !added.is_empty() {
            info!(
                "Dictionary: {} {:?} word(s) from {origin}: {}",
                added.len(),
                status,
                added.join(", ")
            );
            self.sync_settings(added.clone());
        }
        Ok(added)
    }

    pub fn set_status(&self, id: i64, status: DictionaryStatus) -> Result<()> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.connection()?;
        let updated = conn.execute(
            "UPDATE dictionary SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.as_str(), now_ms(), id],
        )?;
        drop(conn);
        if updated == 0 {
            return Err(anyhow!("no dictionary entry {id}"));
        }
        self.sync_settings(Vec::new());
        Ok(())
    }

    /// Accepts or dismisses everything waiting for review.
    pub fn resolve_pending(&self, status: DictionaryStatus) -> Result<u32> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.connection()?;
        let updated = conn.execute(
            "UPDATE dictionary SET status = ?1, updated_at = ?2 WHERE status = 'pending'",
            params![status.as_str(), now_ms()],
        )?;
        drop(conn);
        self.sync_settings(Vec::new());
        Ok(updated as u32)
    }

    /// Changes the spelling of an entry; editing marks it manual.
    pub fn rename(&self, id: i64, word: &str) -> Result<DictionaryEntry> {
        let word = normalize_word(word).ok_or_else(|| anyhow!("the word is empty"))?;
        let key = word_key(&word);
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.connection()?;
        let taken: Option<i64> = conn
            .query_row(
                "SELECT id FROM dictionary WHERE word_key = ?1 AND id != ?2",
                params![key, id],
                |row| row.get(0),
            )
            .optional()?;
        if taken.is_some() {
            return Err(anyhow!("\"{word}\" is already in the dictionary"));
        }
        let updated = conn.execute(
            "UPDATE dictionary SET word = ?1, word_key = ?2, source = 'manual', status = 'active', updated_at = ?3
             WHERE id = ?4",
            params![word, key, now_ms(), id],
        )?;
        if updated == 0 {
            return Err(anyhow!("no dictionary entry {id}"));
        }
        let entry = conn.query_row(
            "SELECT id, word, source, status, origin, created_at, updated_at, last_used_at, use_count
             FROM dictionary WHERE id = ?1",
            params![id],
            Self::map_entry,
        )?;
        drop(conn);
        self.sync_settings(Vec::new());
        Ok(entry)
    }

    pub fn remove(&self, id: i64) -> Result<()> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.connection()?;
        conn.execute("DELETE FROM dictionary WHERE id = ?1", params![id])?;
        drop(conn);
        self.sync_settings(Vec::new());
        Ok(())
    }

    /// Removes every entry, dismissed ones included.
    pub fn clear(&self) -> Result<()> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.connection()?;
        conn.execute("DELETE FROM dictionary", [])?;
        drop(conn);
        self.sync_settings(Vec::new());
        Ok(())
    }

    /// Makes the active words exactly `words` (the old flat-list API):
    /// missing ones are added as manual, absent ones removed.
    pub fn replace_active(&self, words: &[String]) -> Result<()> {
        let wanted: Vec<String> = words.iter().filter_map(|w| normalize_word(w)).collect();
        {
            let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
            let conn = self.connection()?;
            let keys: Vec<String> = wanted.iter().map(|w| word_key(w)).collect();
            let current = Self::active_words_in(&conn)?;
            for word in current {
                if !keys.contains(&word_key(&word)) {
                    conn.execute(
                        "DELETE FROM dictionary WHERE word_key = ?1",
                        params![word_key(&word)],
                    )?;
                }
            }
        }
        self.add_manual(&wanted)?;
        Ok(())
    }

    /// Splits a pasted list (commas or newlines) into manual words.
    pub fn import_text(&self, text: &str) -> Result<Vec<String>> {
        let words: Vec<String> = text
            .split([',', '\n', ';'])
            .map(str::to_string)
            .collect();
        self.add_manual(&words)
    }

    /// The active words, one per line, for a text export.
    pub fn export_text(&self) -> Result<String> {
        let mut words = self.active_words()?;
        words.sort_by_key(|w| w.to_lowercase());
        Ok(words.join("\n"))
    }

    /// Notes that the active words occurring in `text` were just used, so
    /// the most useful words stay at the head of the vocabulary.
    pub fn record_uses(&self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let lowered = text.to_lowercase();
        let result: Result<()> = (|| {
            let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
            let conn = self.connection()?;
            let words = Self::active_words_in(&conn)?;
            let now = now_ms();
            for word in words {
                if contains_term(&lowered, &word.to_lowercase()) {
                    conn.execute(
                        "UPDATE dictionary SET last_used_at = ?1, use_count = use_count + 1 WHERE word_key = ?2",
                        params![now, word_key(&word)],
                    )?;
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            warn!("Dictionary: recording word uses failed: {e:#}");
        }
    }

    /// Mirrors the active words into `custom_words` and tells the UI.
    fn sync_settings(&self, added: Vec<String>) {
        let (words, pending, active) = match self.connection().and_then(|conn| {
            let words = Self::active_words_in(&conn)?;
            let (pending, active) = Self::counts(&conn)?;
            Ok((words, pending, active))
        }) {
            Ok(state) => state,
            Err(e) => {
                warn!("Dictionary: reading the store failed: {e:#}");
                return;
            }
        };

        let mut settings = get_settings(&self.app);
        if settings.custom_words != words {
            settings.custom_words = words;
            write_settings(&self.app, settings);
            let _ = self.app.emit(
                "settings-changed",
                serde_json::json!({ "setting": "custom_words" }),
            );
        }
        if let Err(e) = (DictionaryUpdatedEvent {
            added,
            pending,
            active,
        })
        .emit(&self.app)
        {
            debug!("Dictionary: emitting the update event failed: {e}");
        }
    }
}

/// Whether `term` occurs in `text` as whole words (both lowercased).
fn contains_term(text: &str, term: &str) -> bool {
    let mut from = 0;
    while let Some(found) = text[from..].find(term) {
        let start = from + found;
        let end = start + term.len();
        let before_ok = text[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after_ok = text[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        from = start + term.chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// The dictionary as a system-prompt block for an LLM, or `None` when it is
/// empty. Goes after the user's prompt so that stays a stable, cacheable
/// prefix.
pub fn prompt_block(words: &[String]) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    Some(format!(
        "Known names and terms, spelled the way they should appear: {}",
        words.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_word_matching() {
        assert!(contains_term("we use grafana daily", "grafana"));
        assert!(contains_term("grafana", "grafana"));
        assert!(!contains_term("the grafanas", "grafana"));
        assert!(!contains_term("ungrafana", "grafana"));
        assert!(contains_term("talk to sinead.", "sinead"));
    }

    #[test]
    fn prompt_block_lists_words() {
        assert_eq!(prompt_block(&[]), None);
        let block = prompt_block(&["Handy".into(), "cjpais".into()]).unwrap();
        assert!(block.ends_with("Handy, cjpais"));
    }
}
