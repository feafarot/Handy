//! Meetings and their transcript segments, in their own `meetings.db` next to
//! the history database (separate so its migrations never interleave with
//! history's), plus each meeting's audio under `meetings/<id>/`.

use anyhow::Result;
use log::{info, warn};
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static MIGRATIONS: &[M] = &[
    M::up(
        "CREATE TABLE meetings (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            ended_at INTEGER,
            status TEXT NOT NULL,
            system_audio INTEGER NOT NULL
        );
        CREATE TABLE meeting_segments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
            source TEXT NOT NULL,
            speaker TEXT,
            start_ms INTEGER NOT NULL,
            end_ms INTEGER NOT NULL,
            text TEXT NOT NULL
        );
        CREATE INDEX meeting_segments_by_meeting ON meeting_segments(meeting_id, start_ms);",
    ),
    // Speakers become numbers (named per meeting in meeting_speakers); the old
    // text column was never written.
    M::up(
        "ALTER TABLE meeting_segments DROP COLUMN speaker;
        ALTER TABLE meeting_segments ADD COLUMN speaker INTEGER;
        ALTER TABLE meetings ADD COLUMN speakers_identified INTEGER NOT NULL DEFAULT 0;
        CREATE TABLE meeting_speakers (
            meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
            speaker INTEGER NOT NULL,
            name TEXT,
            PRIMARY KEY (meeting_id, speaker)
        );",
    ),
    M::up("ALTER TABLE meetings ADD COLUMN notes TEXT;"),
];

/// Which side of the call a segment came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The microphone: the user.
    Mic,
    /// Everything the PC played: the other participants.
    System,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Source::Mic => "mic",
            Source::System => "system",
        }
    }

    fn parse(value: &str) -> Self {
        if value == "system" {
            Source::System
        } else {
            Source::Mic
        }
    }

    /// The channel's audio file in a meeting's directory.
    pub fn file_name(self) -> &'static str {
        match self {
            Source::Mic => "mic.wav",
            Source::System => "system.wav",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum MeetingStatus {
    Recording,
    Done,
    /// The app exited while it was recording; audio up to then is kept.
    Interrupted,
}

impl MeetingStatus {
    fn as_str(self) -> &'static str {
        match self {
            MeetingStatus::Recording => "recording",
            MeetingStatus::Done => "done",
            MeetingStatus::Interrupted => "interrupted",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "recording" => MeetingStatus::Recording,
            "interrupted" => MeetingStatus::Interrupted,
            _ => MeetingStatus::Done,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct Meeting {
    pub id: i64,
    pub title: String,
    /// Unix milliseconds.
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub status: MeetingStatus,
    /// Whether system audio (the other participants) was recorded.
    pub system_audio: bool,
    /// Whether the other participants' segments carry speaker numbers.
    pub speakers_identified: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct MeetingSegment {
    pub id: i64,
    pub meeting_id: i64,
    pub source: Source,
    /// Which of the other participants spoke (1-based, see
    /// [`MeetingSpeaker`]), once identified. Always unset for the microphone.
    pub speaker: Option<u32>,
    /// Offsets from the start of the meeting.
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// One of the other participants, as told apart by speaker identification.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct MeetingSpeaker {
    pub speaker: u32,
    /// Set once the user names them.
    pub name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct MeetingDetail {
    pub meeting: Meeting,
    pub segments: Vec<MeetingSegment>,
    pub speakers: Vec<MeetingSpeaker>,
    /// Markdown, once written.
    pub notes: Option<String>,
    /// Speaker identification is running for this meeting right now.
    pub identifying_speakers: bool,
    /// Notes are being written for this meeting right now.
    pub writing_notes: bool,
}

/// A segment cut out of another by speaker identification.
pub struct SplitSegment {
    pub speaker: u32,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// The outcome of speaker identification for one system segment.
pub enum SpeakerAssignment {
    Label {
        segment_id: i64,
        speaker: u32,
    },
    /// The speaker changed within the segment: it is replaced by these.
    Split {
        segment_id: i64,
        parts: Vec<SplitSegment>,
    },
}

pub struct MeetingStore {
    connection: Mutex<Connection>,
    audio_root: PathBuf,
}

impl MeetingStore {
    pub fn open(app_data_dir: &Path) -> Result<Self> {
        let audio_root = app_data_dir.join("meetings");
        std::fs::create_dir_all(&audio_root)?;
        let db_path = app_data_dir.join("meetings.db");
        info!("Opening meetings database at {:?}", db_path);
        let mut connection = Connection::open(db_path)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let migrations = Migrations::new(MIGRATIONS.to_vec());
        #[cfg(debug_assertions)]
        migrations.validate().expect("Invalid meeting migrations");
        migrations.to_latest(&mut connection)?;

        let store = Self {
            connection: Mutex::new(connection),
            audio_root,
        };
        store.mark_interrupted()?;
        Ok(store)
    }

    fn connection(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Meetings still marked as recording were cut off by an exit or crash.
    fn mark_interrupted(&self) -> Result<()> {
        let connection = self.connection();
        let changed = connection.execute(
            "UPDATE meetings SET status = ?1,
                ended_at = started_at + COALESCE(
                    (SELECT MAX(end_ms) FROM meeting_segments WHERE meeting_id = meetings.id), 0)
             WHERE status = ?2",
            params![
                MeetingStatus::Interrupted.as_str(),
                MeetingStatus::Recording.as_str()
            ],
        )?;
        if changed > 0 {
            warn!("Marked {changed} meeting(s) interrupted by the last exit");
        }
        Ok(())
    }

    pub fn audio_dir(&self, meeting_id: i64) -> PathBuf {
        self.audio_root.join(meeting_id.to_string())
    }

    pub fn create(&self, title: &str, started_at: i64, system_audio: bool) -> Result<Meeting> {
        let connection = self.connection();
        connection.execute(
            "INSERT INTO meetings (title, started_at, status, system_audio) VALUES (?1, ?2, ?3, ?4)",
            params![
                title,
                started_at,
                MeetingStatus::Recording.as_str(),
                system_audio
            ],
        )?;
        let id = connection.last_insert_rowid();
        drop(connection);
        std::fs::create_dir_all(self.audio_dir(id))?;
        Ok(Meeting {
            id,
            title: title.to_string(),
            started_at,
            ended_at: None,
            status: MeetingStatus::Recording,
            system_audio,
            speakers_identified: false,
        })
    }

    pub fn finish(&self, meeting_id: i64, ended_at: i64) -> Result<()> {
        self.connection().execute(
            "UPDATE meetings SET status = ?1, ended_at = ?2 WHERE id = ?3",
            params![MeetingStatus::Done.as_str(), ended_at, meeting_id],
        )?;
        Ok(())
    }

    pub fn add_segment(
        &self,
        meeting_id: i64,
        source: Source,
        start_ms: i64,
        end_ms: i64,
        text: &str,
    ) -> Result<MeetingSegment> {
        let connection = self.connection();
        connection.execute(
            "INSERT INTO meeting_segments (meeting_id, source, start_ms, end_ms, text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![meeting_id, source.as_str(), start_ms, end_ms, text],
        )?;
        Ok(MeetingSegment {
            id: connection.last_insert_rowid(),
            meeting_id,
            source,
            speaker: None,
            start_ms,
            end_ms,
            text: text.to_string(),
        })
    }

    pub fn list(&self) -> Result<Vec<Meeting>> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT id, title, started_at, ended_at, status, system_audio, speakers_identified
             FROM meetings ORDER BY started_at DESC",
        )?;
        let meetings = statement
            .query_map([], meeting_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(meetings)
    }

    pub fn get(&self, meeting_id: i64) -> Result<Option<MeetingDetail>> {
        let connection = self.connection();
        let Some(meeting) = connection
            .query_row(
                "SELECT id, title, started_at, ended_at, status, system_audio, speakers_identified
                 FROM meetings WHERE id = ?1",
                [meeting_id],
                meeting_from_row,
            )
            .optional()?
        else {
            return Ok(None);
        };
        let mut statement = connection.prepare(
            "SELECT id, meeting_id, source, speaker, start_ms, end_ms, text
             FROM meeting_segments WHERE meeting_id = ?1 ORDER BY start_ms, id",
        )?;
        let segments = statement
            .query_map([meeting_id], segment_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut statement = connection.prepare(
            "SELECT speaker, name FROM meeting_speakers WHERE meeting_id = ?1 ORDER BY speaker",
        )?;
        let speakers = statement
            .query_map([meeting_id], |row| {
                Ok(MeetingSpeaker {
                    speaker: row.get(0)?,
                    name: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let notes = connection.query_row(
            "SELECT notes FROM meetings WHERE id = ?1",
            [meeting_id],
            |row| row.get(0),
        )?;
        Ok(Some(MeetingDetail {
            meeting,
            segments,
            speakers,
            notes,
            identifying_speakers: false,
            writing_notes: false,
        }))
    }

    /// The other participants' segments, in order.
    pub fn system_segments(&self, meeting_id: i64) -> Result<Vec<MeetingSegment>> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT id, meeting_id, source, speaker, start_ms, end_ms, text
             FROM meeting_segments WHERE meeting_id = ?1 AND source = ?2 ORDER BY start_ms, id",
        )?;
        let segments = statement
            .query_map(
                params![meeting_id, Source::System.as_str()],
                segment_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(segments)
    }

    /// Stores speaker identification's result in one go, replacing an
    /// earlier one (and the names given to its speakers). `speakers` is how
    /// many there are, numbered from 1.
    pub fn set_speakers(
        &self,
        meeting_id: i64,
        speakers: u32,
        assignments: &[SpeakerAssignment],
    ) -> Result<()> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE meeting_segments SET speaker = NULL WHERE meeting_id = ?1",
            [meeting_id],
        )?;
        transaction.execute(
            "DELETE FROM meeting_speakers WHERE meeting_id = ?1",
            [meeting_id],
        )?;
        for speaker in 1..=speakers {
            transaction.execute(
                "INSERT INTO meeting_speakers (meeting_id, speaker) VALUES (?1, ?2)",
                params![meeting_id, speaker],
            )?;
        }
        for assignment in assignments {
            match assignment {
                SpeakerAssignment::Label {
                    segment_id,
                    speaker,
                } => {
                    transaction.execute(
                        "UPDATE meeting_segments SET speaker = ?1 WHERE id = ?2 AND meeting_id = ?3",
                        params![speaker, segment_id, meeting_id],
                    )?;
                }
                SpeakerAssignment::Split { segment_id, parts } => {
                    transaction.execute(
                        "DELETE FROM meeting_segments WHERE id = ?1 AND meeting_id = ?2",
                        params![segment_id, meeting_id],
                    )?;
                    for part in parts {
                        transaction.execute(
                            "INSERT INTO meeting_segments
                                (meeting_id, source, speaker, start_ms, end_ms, text)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            params![
                                meeting_id,
                                Source::System.as_str(),
                                part.speaker,
                                part.start_ms,
                                part.end_ms,
                                part.text
                            ],
                        )?;
                    }
                }
            }
        }
        transaction.execute(
            "UPDATE meetings SET speakers_identified = 1 WHERE id = ?1",
            [meeting_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn set_notes(&self, meeting_id: i64, notes: &str) -> Result<()> {
        self.connection().execute(
            "UPDATE meetings SET notes = ?1 WHERE id = ?2",
            params![notes, meeting_id],
        )?;
        Ok(())
    }

    /// Names one of a meeting's speakers; `None` goes back to the number.
    pub fn rename_speaker(&self, meeting_id: i64, speaker: u32, name: Option<&str>) -> Result<()> {
        self.connection().execute(
            "UPDATE meeting_speakers SET name = ?1 WHERE meeting_id = ?2 AND speaker = ?3",
            params![name, meeting_id, speaker],
        )?;
        Ok(())
    }

    pub fn rename(&self, meeting_id: i64, title: &str) -> Result<()> {
        self.connection().execute(
            "UPDATE meetings SET title = ?1 WHERE id = ?2",
            params![title, meeting_id],
        )?;
        Ok(())
    }

    pub fn delete(&self, meeting_id: i64) -> Result<()> {
        self.connection()
            .execute("DELETE FROM meetings WHERE id = ?1", [meeting_id])?;
        let dir = self.audio_dir(meeting_id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }
}

fn meeting_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Meeting> {
    Ok(Meeting {
        id: row.get(0)?,
        title: row.get(1)?,
        started_at: row.get(2)?,
        ended_at: row.get(3)?,
        status: MeetingStatus::parse(&row.get::<_, String>(4)?),
        system_audio: row.get(5)?,
        speakers_identified: row.get(6)?,
    })
}

fn segment_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MeetingSegment> {
    Ok(MeetingSegment {
        id: row.get(0)?,
        meeting_id: row.get(1)?,
        source: Source::parse(&row.get::<_, String>(2)?),
        speaker: row.get(3)?,
        start_ms: row.get(4)?,
        end_ms: row.get(5)?,
        text: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meetings_round_trip_and_interrupted_ones_are_closed_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let store = MeetingStore::open(dir.path()).unwrap();
        let meeting = store.create("Standup", 1_000, true).unwrap();
        assert!(store.audio_dir(meeting.id).is_dir());
        store
            .add_segment(meeting.id, Source::System, 5_000, 7_000, "second")
            .unwrap();
        store
            .add_segment(meeting.id, Source::Mic, 1_000, 3_000, "first")
            .unwrap();

        let detail = store.get(meeting.id).unwrap().unwrap();
        let texts: Vec<_> = detail.segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["first", "second"]);
        assert_eq!(detail.segments[1].source, Source::System);

        // Reopening with the meeting still "recording" means the app died.
        drop(store);
        let store = MeetingStore::open(dir.path()).unwrap();
        let meeting = store.get(meeting.id).unwrap().unwrap().meeting;
        assert_eq!(meeting.status, MeetingStatus::Interrupted);
        assert_eq!(meeting.ended_at, Some(1_000 + 7_000));

        store.rename(meeting.id, "Planning").unwrap();

        // Speaker identification labels one segment and splits another.
        let mixed = store
            .add_segment(meeting.id, Source::System, 8_000, 12_000, "a b")
            .unwrap();
        let system = store.system_segments(meeting.id).unwrap();
        assert_eq!(system.len(), 2);
        store
            .set_speakers(
                meeting.id,
                2,
                &[
                    SpeakerAssignment::Label {
                        segment_id: system[0].id,
                        speaker: 1,
                    },
                    SpeakerAssignment::Split {
                        segment_id: mixed.id,
                        parts: vec![
                            SplitSegment {
                                speaker: 1,
                                start_ms: 8_000,
                                end_ms: 10_000,
                                text: "a".into(),
                            },
                            SplitSegment {
                                speaker: 2,
                                start_ms: 10_000,
                                end_ms: 12_000,
                                text: "b".into(),
                            },
                        ],
                    },
                ],
            )
            .unwrap();
        store.rename_speaker(meeting.id, 2, Some("Dana")).unwrap();
        let detail = store.get(meeting.id).unwrap().unwrap();
        assert!(detail.meeting.speakers_identified);
        let labelled: Vec<_> = detail
            .segments
            .iter()
            .map(|s| (s.text.as_str(), s.speaker))
            .collect();
        assert_eq!(
            labelled,
            [
                ("first", None),
                ("second", Some(1)),
                ("a", Some(1)),
                ("b", Some(2))
            ]
        );
        let names: Vec<_> = detail.speakers.iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, [None, Some("Dana".to_string())]);
        assert_eq!(detail.notes, None);
        store.set_notes(meeting.id, "## Summary").unwrap();
        let detail = store.get(meeting.id).unwrap().unwrap();
        assert_eq!(detail.notes.as_deref(), Some("## Summary"));

        assert_eq!(store.list().unwrap()[0].title, "Planning");
        store.delete(meeting.id).unwrap();
        assert!(store.get(meeting.id).unwrap().is_none());
        assert!(!store.audio_dir(meeting.id).exists());
    }
}
