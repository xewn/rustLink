use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, RwLock},
};

/// A single process owns a config path. Mutations commit on disk before publication.
pub struct ConfigStore {
    path: PathBuf,
    data: RwLock<Value>,
    writer: Mutex<()>,
    _lock: File,
}
impl ConfigStore {
    pub fn open(path: &Path) -> Result<Self> {
        let path = std::path::absolute(path)?;
        let parent = path.parent().context("config parent")?;
        fs::create_dir_all(parent)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("json.lock"))?;
        lock.try_lock_exclusive()
            .context("another RustLink instance owns this configuration")?;
        let v = Self::load_or_recover(&path)?;
        Ok(Self {
            path,
            data: RwLock::new(v),
            writer: Mutex::new(()),
            _lock: lock,
        })
    }
    fn read(path: &Path) -> Result<Value> {
        let v: Value = serde_json::from_slice(&fs::read(path)?)?;
        ensure!(v.is_object(), "config must be an object");
        Ok(v)
    }
    fn load_or_recover(path: &Path) -> Result<Value> {
        if path.exists() {
            match Self::read(path) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    // Permission errors are not evidence of corrupt JSON.
                    let raw = fs::read(path).with_context(|| format!("cannot read config: {e}"))?;
                    let evidence =
                        path.with_extension(format!("json.corrupt.{}", crate::model::timestamp()));
                    fs::write(evidence, raw)?;
                }
            }
        }
        let value = Self::read(&path.with_extension("json.bak"))
            .unwrap_or_else(|_| crate::model::default_config());
        Self::atomic_write(path, &value)?;
        Ok(value)
    }
    fn atomic_write(path: &Path, value: &Value) -> Result<()> {
        let mut temp = tempfile::NamedTempFile::new_in(path.parent().context("config parent")?)?;
        serde_json::to_writer_pretty(&mut temp, value)?;
        temp.write_all(b"\n")?;
        temp.as_file().sync_all()?;
        temp.persist(path).map_err(|e| e.error)?;
        #[cfg(unix)]
        File::open(path.parent().unwrap())?.sync_all()?;
        Ok(())
    }
    pub fn snapshot(&self) -> Value {
        self.data.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn update<T>(&self, mutation: impl FnOnce(&mut Value) -> Result<T>) -> Result<T> {
        self.update_if(|value| mutation(value).map(|result| (result, true)))
    }
    pub fn update_if<T>(
        &self,
        mutation: impl FnOnce(&mut Value) -> Result<(T, bool)>,
    ) -> Result<T> {
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let previous = self.snapshot();
        let mut next = previous.clone();
        let (result, commit) = mutation(&mut next)?;
        if !commit {
            return Ok(result);
        }
        Self::atomic_write(&self.path.with_extension("json.bak"), &previous)?;
        Self::atomic_write(&self.path, &next)?;
        *self.data.write().unwrap_or_else(|e| e.into_inner()) = next;
        Ok(result)
    }
    pub fn save(&self) -> Result<()> {
        self.update(|_| Ok(()))
    }
    pub fn reload(&self) -> Result<()> {
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let next = Self::load_or_recover(&self.path)?;
        *self.data.write().unwrap_or_else(|e| e.into_inner()) = next;
        Ok(())
    }
}
