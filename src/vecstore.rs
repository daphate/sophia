//! Semantic vector store for dialog memory.
//!
//! Uses `fastembed` for local embeddings (multilingual-e5-small, 384 dim)
//! and `usearch` for HNSW vector index. Metadata stored in SQLite.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use rusqlite::Connection;
use tracing::{debug, error, info, warn};
use usearch::ffi::{IndexOptions, MetricKind, ScalarKind};

/// Embedding dimension for multilingual-e5-small.
const EMBED_DIM: usize = 384;

/// Maximum results to return from semantic search.
const DEFAULT_TOP_K: usize = 10;

/// A single search result with metadata.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub text: String,
    pub role: String,
    pub user_id: i64,
    pub timestamp: String,
    pub score: f32,
}

/// Semantic vector store combining usearch index with SQLite metadata.
pub struct VecStore {
    index: usearch::Index,
    db: Mutex<Connection>,
    embedder: Mutex<TextEmbedding>,
    next_key: Mutex<u64>,
}

impl VecStore {
    /// Initialize or open the vector store.
    ///
    /// - `db_path`: path to SQLite database for metadata
    /// - `index_path`: path to usearch index file
    pub fn new(db_path: &Path, index_path: &Path) -> Result<Self> {
        // Initialize embedding model
        info!("Loading embedding model (multilingual-e5-small)...");
        let embedder = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::MultilingualE5Small).with_show_download_progress(true),
        )
        .context("Failed to initialize embedding model")?;
        info!("Embedding model loaded");

        // Initialize usearch index
        let opts = IndexOptions {
            dimensions: EMBED_DIM,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F32,
            ..Default::default()
        };
        let index = usearch::new_index(&opts).context("Failed to create usearch index")?;

        // Try to load existing index
        if index_path.exists() {
            index
                .load(index_path.to_str().unwrap_or(""))
                .context("Failed to load usearch index")?;
            info!(
                "Loaded existing usearch index ({} vectors)",
                index.size()
            );
        } else {
            // Reserve initial capacity
            index
                .reserve(10_000)
                .context("Failed to reserve index capacity")?;
            info!("Created new usearch index");
        }

        // Initialize SQLite
        let db = Connection::open(db_path).context("Failed to open vecstore database")?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS chunks (
                key     INTEGER PRIMARY KEY,
                text    TEXT NOT NULL,
                role    TEXT NOT NULL,
                user_id INTEGER NOT NULL,
                ts      TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_chunks_user ON chunks(user_id);
            CREATE INDEX IF NOT EXISTS idx_chunks_ts ON chunks(ts);",
        )?;

        // Determine next key
        let max_key: u64 = db
            .query_row("SELECT COALESCE(MAX(key), 0) FROM chunks", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);
        let next_key = max_key + 1;

        info!("VecStore initialized: {} chunks in DB", max_key);

        Ok(Self {
            index,
            db: Mutex::new(db),
            embedder: Mutex::new(embedder),
            next_key: Mutex::new(next_key),
        })
    }

    /// Add a text chunk to the store.
    pub fn add(&self, text: &str, role: &str, user_id: i64, timestamp: &str) -> Result<()> {
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.len() < 5 {
            return Ok(()); // skip tiny chunks
        }

        // Embed
        let embedding = self.embed_one(trimmed)?;

        // Get next key
        let key = {
            let mut k = self.next_key.lock().unwrap();
            let current = *k;
            *k += 1;
            current
        };

        // Grow index if needed
        let capacity = self.index.capacity();
        let size = self.index.size();
        if size + 1 >= capacity {
            self.index
                .reserve(capacity + 10_000)
                .context("Failed to grow index")?;
        }

        // Add to usearch
        self.index
            .add(key, &embedding)
            .context("Failed to add vector to index")?;

        // Add metadata to SQLite
        {
            let db = self.db.lock().unwrap();
            db.execute(
                "INSERT INTO chunks (key, text, role, user_id, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![key, trimmed, role, user_id, timestamp],
            )?;
        }

        debug!(
            "Indexed chunk key={} role={} len={} user={}",
            key,
            role,
            trimmed.len(),
            user_id
        );

        Ok(())
    }

    /// Search for semantically similar chunks.
    pub fn search(&self, query: &str, top_k: Option<usize>) -> Result<Vec<SearchResult>> {
        let k = top_k.unwrap_or(DEFAULT_TOP_K);

        if self.index.size() == 0 {
            return Ok(vec![]);
        }

        let embedding = self.embed_one(query)?;

        let results = self
            .index
            .search(&embedding, k)
            .context("usearch search failed")?;

        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare_cached(
            "SELECT text, role, user_id, ts FROM chunks WHERE key = ?1",
        )?;

        let mut out = Vec::with_capacity(results.keys.len());
        for (key, distance) in results.keys.iter().zip(results.distances.iter()) {
            match stmt.query_row([key], |row| {
                Ok(SearchResult {
                    text: row.get(0)?,
                    role: row.get(1)?,
                    user_id: row.get(2)?,
                    timestamp: row.get(3)?,
                    score: 1.0 - distance, // cosine distance → similarity
                })
            }) {
                Ok(r) => out.push(r),
                Err(e) => warn!("Chunk key={} not in DB: {}", key, e),
            }
        }

        Ok(out)
    }

    /// Save the index to disk.
    pub fn save(&self, index_path: &Path) -> Result<()> {
        self.index
            .save(index_path.to_str().unwrap_or(""))
            .context("Failed to save usearch index")?;
        debug!("Index saved ({} vectors)", self.index.size());
        Ok(())
    }

    /// Get count of stored vectors.
    pub fn len(&self) -> usize {
        self.index.size()
    }

    /// Embed a single text, returning the vector.
    fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut embedder = self.embedder.lock().unwrap();
        let embeddings = embedder
            .embed(vec![text.to_string()], None)
            .context("Embedding failed")?;

        embeddings
            .into_iter()
            .next()
            .context("No embedding returned")
    }

    /// Batch-index existing dialog files (for initial migration).
    pub fn index_dialog_file(&self, path: &Path, user_id: i64) -> Result<usize> {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        if content.is_empty() {
            return Ok(0);
        }

        let mut count = 0;
        for block in content.split("\n\n") {
            let block = block.trim();
            if block.is_empty() {
                continue;
            }

            // Parse format: **Role** [HH:MM:SS]: text
            let (role, timestamp, text) = parse_dialog_entry(block);
            if text.is_empty() {
                continue;
            }

            if let Err(e) = self.add(&text, &role, user_id, &timestamp) {
                error!("Failed to index chunk: {}", e);
                continue;
            }
            count += 1;
        }

        Ok(count)
    }
}

/// Parse a dialog entry in the format: **Role** [HH:MM:SS]: text
fn parse_dialog_entry(entry: &str) -> (String, String, String) {
    // Try to parse **Role** [timestamp]: text
    let re = regex::Regex::new(r"^\*\*(\w+)\*\*\s*\[([^\]]+)\]:\s*(.*)$").unwrap();
    if let Some(caps) = re.captures(entry) {
        let role = caps[1].to_string();
        let ts = caps[2].to_string();
        let text = caps[3].to_string();
        return (role, ts, text);
    }

    // Fallback: treat as plain text
    ("Unknown".to_string(), String::new(), entry.to_string())
}

/// Format search results as context for the system prompt.
pub fn format_search_context(results: &[SearchResult], max_chars: usize) -> String {
    if results.is_empty() {
        return String::new();
    }

    let mut parts = Vec::new();
    let mut total = 0;

    for r in results {
        if total + r.text.len() > max_chars {
            break;
        }
        let line = format!("[{}] {}: {}", r.timestamp, r.role, r.text);
        total += line.len();
        parts.push(line);
    }

    if parts.is_empty() {
        return String::new();
    }

    format!(
        "# Relevant Past Context (semantic search)\n{}",
        parts.join("\n")
    )
}
