use anyhow::Result;
use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

pub struct LogStore {
    path: PathBuf,
    max_bytes: u64,
    state: Mutex<u8>,
}
fn severity(s: &str) -> u8 {
    match s.to_uppercase().as_str() {
        "DEBUG" => 10,
        "INFO" => 20,
        "WARNING" => 30,
        "ERROR" => 40,
        _ => 20,
    }
}
impl LogStore {
    pub fn open(dir: &Path, max_bytes: u64) -> Result<Self> {
        fs::create_dir_all(dir)?;
        Ok(Self {
            path: dir.join("gateway.log"),
            max_bytes,
            state: Mutex::new(20),
        })
    }
    pub fn set_level(&self, level: &str) {
        if matches!(
            level.to_uppercase().as_str(),
            "DEBUG" | "INFO" | "WARNING" | "ERROR"
        ) {
            *self.state.lock().unwrap_or_else(|e| e.into_inner()) = severity(level);
        }
    }
    pub fn write(&self, level: &str, message: &str) -> Result<()> {
        let threshold = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Original handlers have an INFO floor even when the logger is DEBUG.
        if severity(level) < (*threshold).max(20) {
            return Ok(());
        }
        let line = format!(
            "[{}] [{}] [gateway] {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            level.to_uppercase(),
            message
        );
        if fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0) + line.len() as u64
            > self.max_bytes
        {
            for i in (1..=5).rev() {
                let dest = self.path.with_extension(format!("log.{i}"));
                let prev = if i == 1 {
                    self.path.clone()
                } else {
                    self.path.with_extension(format!("log.{}", i - 1))
                };
                if dest.exists() {
                    fs::remove_file(&dest)?;
                }
                if prev.exists() {
                    fs::rename(prev, dest)?;
                }
            }
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        Ok(())
    }
    pub fn read(&self, level: Option<&str>, lines: i64) -> Result<Vec<String>> {
        let _guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let all = BufReader::new(std::fs::File::open(&self.path)?);
        // Python [-0:] means all; negative N means skip -N initial lines.
        let mut recent = VecDeque::new();
        for (index, line) in all.lines().enumerate() {
            let line = line?;
            if lines < 0 && index < lines.unsigned_abs() as usize {
                continue;
            }
            recent.push_back(line);
            if lines > 0 && recent.len() > lines as usize {
                recent.pop_front();
            }
        }
        let tag = level
            .filter(|s| !s.is_empty())
            .map(|s| format!("[{}]", s.to_uppercase()));
        Ok(recent
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && tag.as_ref().is_none_or(|tag| s.contains(tag)))
            .collect())
    }
}
