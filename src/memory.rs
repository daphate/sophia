use std::fs;
use std::path::Path;

use chrono::Utc;
use regex::Regex;
use tracing::info;

use crate::config;

/// Read a file safely, returning empty string if missing.
fn read_file_safe(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

// --- Memory ---

pub fn read_memory() -> String {
    let raw = read_file_safe(&config::memory_file());
    deduplicate_memory(&raw)
}

pub fn append_memory(text: &str) {
    let dir = config::memory_dir();
    let _ = fs::create_dir_all(&dir);

    let current = read_memory();

    // Remove placeholder if present
    let current = if current.contains("No memories stored yet.") {
        "# Memory\n\n".to_string()
    } else {
        current
    };

    // Dedup check
    let new_norm = normalize_fact(text);
    for line in current.lines() {
        let stripped = line.trim();
        if let Some(fact) = stripped.strip_prefix("- ") {
            let existing_norm = normalize_fact(fact);
            if !existing_norm.is_empty() && !new_norm.is_empty() && existing_norm == new_norm {
                info!("Skipping duplicate memory entry: {}", &text[..text.len().min(80)]);
                return;
            }
        }
    }

    let timestamp = Utc::now().format("%Y-%m-%d %H:%M UTC");
    let entry = format!("- [{}] {}\n", timestamp, text.trim());
    let new_content = format!("{}\n{}", current.trim_end(), entry);
    let _ = fs::write(config::memory_file(), new_content);
    info!("Memory updated: {}", &text[..text.len().min(80)]);
}

pub fn clear_memory() {
    let _ = fs::create_dir_all(config::memory_dir());
    let _ = fs::write(config::memory_file(), "# Memory\n\nNo memories stored yet.\n");
}

pub fn extract_memory_updates(response: &str) -> (String, Vec<String>) {
    let re = Regex::new(r"\[MEMORY_UPDATE\](.*?)\[/MEMORY_UPDATE\]").unwrap();
    let mut updates = Vec::new();
    for cap in re.captures_iter(response) {
        let text = cap[1].trim().to_string();
        if !text.is_empty() {
            updates.push(text);
        }
    }
    let cleaned = re.replace_all(response, "").trim().to_string();
    (cleaned, updates)
}

// --- System prompt ---

pub fn build_system_prompt(recent_dialog: &str, semantic_context: &str) -> String {
    let agents = read_file_safe(&config::agents_file());
    let identity = read_file_safe(&config::identity_file());
    let soul = read_file_safe(&config::soul_file());
    let user_ctx = read_file_safe(&config::user_file());
    let tools = read_file_safe(&config::tools_file());
    let instructions_memory = read_file_safe(&config::instructions_memory_file());
    let mut memory = read_memory();

    // Truncate memory if too long (8 KiB is fine with 1M context)
    if memory.len() > 8192 {
        let mut start = memory.len() - 8192;
        while !memory.is_char_boundary(start) {
            start += 1;
        }
        memory = format!(
            "# Memory\n…(older entries truncated)\n{}",
            &memory[start..]
        );
    }

    let mut parts = Vec::new();
    let agents_trimmed = agents.trim();
    if !agents_trimmed.is_empty() {
        parts.push(agents_trimmed.to_string());
    }
    let identity_trimmed = identity.trim();
    if !identity_trimmed.is_empty() {
        parts.push(identity_trimmed.to_string());
    }
    let soul_trimmed = soul.trim();
    if !soul_trimmed.is_empty() {
        parts.push(soul_trimmed.to_string());
    }
    let user_trimmed = user_ctx.trim();
    if !user_trimmed.is_empty() && !user_trimmed.contains("(your name)") {
        parts.push(user_trimmed.to_string());
    }
    let tools_trimmed = tools.trim();
    if !tools_trimmed.is_empty() {
        parts.push(tools_trimmed.to_string());
    }
    let instr_mem_trimmed = instructions_memory.trim();
    if !instr_mem_trimmed.is_empty() {
        parts.push(instr_mem_trimmed.to_string());
    }
    let memory_trimmed = memory.trim();
    if !memory_trimmed.is_empty() && !memory_trimmed.contains("No memories stored yet.") {
        parts.push(memory_trimmed.to_string());
    }
    if !semantic_context.is_empty() {
        parts.push(semantic_context.to_string());
    }
    if !recent_dialog.is_empty() {
        parts.push(format!("# Recent Conversation\n{}", recent_dialog));
    }
    parts.push(
        "To save important facts, append at end of response: \
         [MEMORY_UPDATE]fact[/MEMORY_UPDATE] (hidden from user)."
            .to_string(),
    );

    parts.join("\n\n")
}

// --- Dialog persistence ---

pub fn append_dialog(user_id: i64, role: &str, text: &str) {
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let user_dir = config::dialogs_dir().join(user_id.to_string());
    let _ = fs::create_dir_all(&user_dir);
    let path = user_dir.join(format!("{}.md", today));

    let timestamp = Utc::now().format("%H:%M:%S");
    let entry = format!("**{}** [{}]: {}\n\n", role, timestamp, text);
    let mut content = read_file_safe(&path);
    content.push_str(&entry);
    let _ = fs::write(&path, content);
}

pub fn load_recent_dialog(user_id: i64, max_turns: usize, max_total_chars: usize) -> String {
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let path = config::dialogs_dir()
        .join(user_id.to_string())
        .join(format!("{}.md", today));

    if !path.exists() {
        return String::new();
    }

    let content = read_file_safe(&path);
    let turns: Vec<&str> = content
        .split("\n\n")
        .filter(|t| !t.trim().is_empty())
        .collect();

    let recent: Vec<String> = turns
        .iter()
        .rev()
        .take(max_turns)
        .rev()
        .map(|t| {
            if t.len() > 300 {
                let mut end = 300;
                while !t.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}…(truncated)", &t[..end])
            } else {
                t.to_string()
            }
        })
        .collect();

    // Enforce total size cap
    let mut result: Vec<String> = recent;
    let mut joined = result.join("\n\n");
    while joined.len() > max_total_chars && result.len() > 4 {
        result.remove(0);
        joined = result.join("\n\n");
    }

    joined
}

// --- Dedup helpers ---

fn deduplicate_memory(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut non_entry_lines: Vec<String> = Vec::new();
    let mut entries: Vec<String> = Vec::new();

    for line in lines {
        let stripped = line.trim();
        if stripped.starts_with("- ") {
            entries.push(line.to_string());
        } else {
            if !entries.is_empty() {
                let deduped = keep_last_unique(&entries);
                non_entry_lines.extend(deduped);
                entries.clear();
            }
            non_entry_lines.push(line.to_string());
        }
    }
    if !entries.is_empty() {
        let deduped = keep_last_unique(&entries);
        non_entry_lines.extend(deduped);
    }

    non_entry_lines.join("\n")
}

fn keep_last_unique(entries: &[String]) -> Vec<String> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, line) in entries.iter().enumerate() {
        let fact = line.trim().strip_prefix("- ").unwrap_or("");
        let norm = normalize_fact(fact);
        if !norm.is_empty() {
            seen.insert(norm, i);
        }
    }
    let last_indices: std::collections::HashSet<usize> = seen.into_values().collect();
    entries
        .iter()
        .enumerate()
        .filter(|(i, _)| last_indices.contains(i))
        .map(|(_, line)| line.clone())
        .collect()
}

fn normalize_fact(text: &str) -> String {
    let re = Regex::new(r"\[\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}\s*\w*\]\s*").unwrap();
    let s = re.replace_all(text, "");
    s.trim().to_lowercase()
}
