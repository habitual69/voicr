use anyhow::Result;
use chrono::{DateTime, Local, Utc};
use log::{debug, error, info};
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::audio_toolkit::save_wav_file;

static MIGRATIONS: &[M] = &[
    M::up(
        "CREATE TABLE IF NOT EXISTS transcription_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_name TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            saved BOOLEAN NOT NULL DEFAULT 0,
            title TEXT NOT NULL,
            transcription_text TEXT NOT NULL
        );",
    ),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_processed_text TEXT;"),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_process_prompt TEXT;"),
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub file_name: String,
    pub timestamp: i64,
    pub saved: bool,
    pub title: String,
    pub transcription_text: String,
    pub post_processed_text: Option<String>,
    pub post_process_prompt: Option<String>,
}

pub struct HistoryManager {
    recordings_dir: PathBuf,
    db_path: PathBuf,
    history_limit: usize,
    retention: crate::config::RecordingRetention,
}

impl HistoryManager {
    pub fn new(
        recordings_dir: PathBuf,
        db_path: PathBuf,
        history_limit: usize,
        retention: crate::config::RecordingRetention,
    ) -> Result<Self> {
        if !recordings_dir.exists() {
            fs::create_dir_all(&recordings_dir)?;
            debug!("Created recordings directory: {:?}", recordings_dir);
        }

        let manager = Self {
            recordings_dir,
            db_path,
            history_limit,
            retention,
        };

        manager.init_database()?;
        Ok(manager)
    }

    fn init_database(&self) -> Result<()> {
        info!("Initializing database at {:?}", self.db_path);
        let mut conn = Connection::open(&self.db_path)?;
        let migrations = Migrations::new(MIGRATIONS.to_vec());

        #[cfg(debug_assertions)]
        migrations.validate().expect("Invalid migrations");

        migrations.to_latest(&mut conn)?;
        Ok(())
    }

    fn get_connection(&self) -> Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    pub async fn save_transcription(
        &self,
        audio_samples: Vec<f32>,
        transcription_text: String,
        post_processed_text: Option<String>,
        post_process_prompt: Option<String>,
    ) -> Result<()> {
        let timestamp = Utc::now().timestamp();
        let file_name = format!("voicr-{}.wav", timestamp);
        let title = self.format_timestamp_title(timestamp);

        let file_path = self.recordings_dir.join(&file_name);
        save_wav_file(file_path, &audio_samples).await?;

        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO transcription_history (file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![file_name, timestamp, false, title, transcription_text, post_processed_text, post_process_prompt],
        )?;

        debug!("Saved transcription to history");
        self.cleanup_old_entries()?;
        Ok(())
    }

    pub fn cleanup_old_entries(&self) -> Result<()> {
        match &self.retention {
            crate::config::RecordingRetention::Never => Ok(()),
            crate::config::RecordingRetention::PreserveLimit => {
                self.cleanup_by_count(self.history_limit)
            }
            crate::config::RecordingRetention::Days3 => {
                self.cleanup_by_age(3 * 24 * 60 * 60)
            }
            crate::config::RecordingRetention::Weeks2 => {
                self.cleanup_by_age(2 * 7 * 24 * 60 * 60)
            }
            crate::config::RecordingRetention::Months3 => {
                self.cleanup_by_age(3 * 30 * 24 * 60 * 60)
            }
        }
    }

    fn cleanup_by_count(&self, limit: usize) -> Result<()> {
        if limit == 0 {
            return Ok(());
        }

        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, file_name FROM transcription_history WHERE saved = 0 ORDER BY timestamp DESC"
        )?;

        let entries: Vec<(i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        if entries.len() > limit {
            for (id, file_name) in &entries[limit..] {
                conn.execute(
                    "DELETE FROM transcription_history WHERE id = ?1",
                    params![id],
                )?;
                let file_path = self.recordings_dir.join(file_name);
                if file_path.exists() {
                    if let Err(e) = fs::remove_file(&file_path) {
                        error!("Failed to delete recording {}: {}", file_name, e);
                    }
                }
            }
        }

        Ok(())
    }

    fn cleanup_by_age(&self, max_age_secs: i64) -> Result<()> {
        let cutoff = Utc::now().timestamp() - max_age_secs;
        let conn = self.get_connection()?;

        let mut stmt = conn.prepare(
            "SELECT id, file_name FROM transcription_history WHERE saved = 0 AND timestamp < ?1",
        )?;

        let entries: Vec<(i64, String)> = stmt
            .query_map(params![cutoff], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        for (id, file_name) in &entries {
            conn.execute(
                "DELETE FROM transcription_history WHERE id = ?1",
                params![id],
            )?;
            let file_path = self.recordings_dir.join(file_name);
            if file_path.exists() {
                if let Err(e) = fs::remove_file(&file_path) {
                    error!("Failed to delete recording {}: {}", file_name, e);
                }
            }
        }

        Ok(())
    }

    pub async fn get_history_entries(&self) -> Result<Vec<HistoryEntry>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt \
             FROM transcription_history ORDER BY timestamp DESC"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(HistoryEntry {
                id: row.get("id")?,
                file_name: row.get("file_name")?,
                timestamp: row.get("timestamp")?,
                saved: row.get("saved")?,
                title: row.get("title")?,
                transcription_text: row.get("transcription_text")?,
                post_processed_text: row.get("post_processed_text")?,
                post_process_prompt: row.get("post_process_prompt")?,
            })
        })?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    pub async fn get_entry_by_id(&self, id: i64) -> Result<Option<HistoryEntry>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt \
             FROM transcription_history WHERE id = ?1",
        )?;

        let entry = stmt
            .query_row([id], |row| {
                Ok(HistoryEntry {
                    id: row.get("id")?,
                    file_name: row.get("file_name")?,
                    timestamp: row.get("timestamp")?,
                    saved: row.get("saved")?,
                    title: row.get("title")?,
                    transcription_text: row.get("transcription_text")?,
                    post_processed_text: row.get("post_processed_text")?,
                    post_process_prompt: row.get("post_process_prompt")?,
                })
            })
            .optional()?;

        Ok(entry)
    }

    pub async fn toggle_saved_status(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;
        let current_saved: bool = conn.query_row(
            "SELECT saved FROM transcription_history WHERE id = ?1",
            params![id],
            |row| row.get("saved"),
        )?;
        conn.execute(
            "UPDATE transcription_history SET saved = ?1 WHERE id = ?2",
            params![!current_saved, id],
        )?;
        Ok(())
    }

    pub async fn delete_entry(&self, id: i64) -> Result<()> {
        if let Some(entry) = self.get_entry_by_id(id).await? {
            let file_path = self.recordings_dir.join(&entry.file_name);
            if file_path.exists() {
                if let Err(e) = fs::remove_file(&file_path) {
                    error!("Failed to delete audio file: {}", e);
                }
            }
        }

        let conn = self.get_connection()?;
        conn.execute(
            "DELETE FROM transcription_history WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn get_audio_file_path(&self, file_name: &str) -> PathBuf {
        self.recordings_dir.join(file_name)
    }

    fn format_timestamp_title(&self, timestamp: i64) -> String {
        if let Some(utc_datetime) = DateTime::from_timestamp(timestamp, 0) {
            let local_datetime = utc_datetime.with_timezone(&Local);
            local_datetime.format("%B %e, %Y - %l:%M%p").to_string()
        } else {
            format!("Recording {}", timestamp)
        }
    }
}
