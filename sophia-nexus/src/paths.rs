use std::path::{Path, PathBuf};

pub fn data_dir() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("data")
}

pub fn instructions_dir(data: &Path) -> PathBuf {
    data.join("instructions")
}

pub fn memory_file(data: &Path) -> PathBuf {
    data.join("memory").join("MEMORY.md")
}

pub fn dialogs_dir(data: &Path) -> PathBuf {
    data.join("dialogs")
}

pub fn users_dir(data: &Path) -> PathBuf {
    data.join("users")
}

pub fn outbox_dir(data: &Path) -> PathBuf {
    data.join("outbox")
}

pub fn vecstore_db(data: &Path) -> PathBuf {
    data.join("vecstore.db")
}
