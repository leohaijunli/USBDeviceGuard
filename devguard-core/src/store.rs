//! Loading, validating, backing up and atomically writing `devices.d`.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::config::{validate_set, DeviceConfig, ValidationIssue};
use crate::paths::Paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigLoadError {
    pub file: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct LoadedStore {
    pub devices: Vec<DeviceConfig>,
    pub errors: Vec<ConfigLoadError>,
    pub issues: Vec<ValidationIssue>,
}

impl LoadedStore {
    pub fn get(&self, name: &str) -> Option<&DeviceConfig> {
        self.devices.iter().find(|device| device.name == name)
    }
}

#[derive(Debug, Clone)]
pub struct SaveOutcome {
    pub config: DeviceConfig,
    pub path: PathBuf,
    pub backup: Option<PathBuf>,
    pub warnings: Vec<ValidationIssue>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("读取配置失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("配置校验失败：{message}")]
    Invalid {
        message: String,
        issues: Vec<ValidationIssue>,
    },
    #[error("设备 `{0}` 不存在")]
    NotFound(String),
    #[error("设备名 `{0}` 无效")]
    InvalidName(String),
}

#[derive(Debug, Clone)]
pub struct DeviceStore {
    pub paths: Paths,
}

impl DeviceStore {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    pub fn load(&self) -> Result<LoadedStore, StoreError> {
        let dir = self.paths.devices_dir();
        if !dir.exists() {
            return Ok(LoadedStore {
                devices: Vec::new(),
                errors: Vec::new(),
                issues: Vec::new(),
            });
        }

        let mut files: Vec<PathBuf> = fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .collect();
        files.sort();

        let mut devices = Vec::new();
        let mut errors = Vec::new();
        for file in files {
            let mut text = String::new();
            match File::open(&file) {
                Ok(mut handle) => {
                    if let Err(error) = handle.read_to_string(&mut text) {
                        errors.push(ConfigLoadError {
                            file,
                            message: format!("读取失败：{error}"),
                        });
                        continue;
                    }
                }
                Err(error) => {
                    errors.push(ConfigLoadError {
                        file,
                        message: format!("打开失败：{error}"),
                    });
                    continue;
                }
            }
            match DeviceConfig::from_toml(&text) {
                Ok(config) => {
                    if file.file_stem().and_then(|s| s.to_str()) != Some(config.name.as_str()) {
                        errors.push(ConfigLoadError {
                            file: file.clone(),
                            message: format!(
                                "文件名与 name = \"{}\" 不一致；改名为 {}.toml 或修改 name",
                                config.name, config.name
                            ),
                        });
                    }
                    devices.push(config);
                }
                Err(error) => errors.push(ConfigLoadError {
                    file,
                    message: format!("TOML 解析失败：{error}"),
                }),
            }
        }
        devices.sort_by(|a, b| a.name.cmp(&b.name));
        let issues = validate_set(&devices);
        Ok(LoadedStore {
            devices,
            errors,
            issues,
        })
    }

    pub fn get(&self, name: &str) -> Result<DeviceConfig, StoreError> {
        let loaded = self.load()?;
        loaded
            .get(name)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(name.to_owned()))
    }

    /// Validate against every other device, back up the old file and write the
    /// new one with a temp-file + rename.  A failed rename leaves the old file
    /// untouched.
    pub fn save(
        &self,
        config: &DeviceConfig,
        _note: Option<&str>,
    ) -> Result<SaveOutcome, StoreError> {
        if !config.name.is_empty()
            && !config
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(StoreError::InvalidName(config.name.clone()));
        }
        let loaded = self.load()?;
        let mut candidate = loaded.devices.clone();
        candidate.retain(|device| device.name != config.name);
        candidate.push(config.clone());
        let issues = validate_set(&candidate);
        if let Some(issue) = issues
            .iter()
            .find(|issue| issue.severity == crate::check::Severity::Error)
        {
            return Err(StoreError::Invalid {
                message: format!("{}：{}。{}", issue.field, issue.message, issue.next_step),
                issues: issues.clone(),
            });
        }
        let warnings = issues
            .into_iter()
            .filter(|issue| issue.severity == crate::check::Severity::Warning)
            .collect();

        fs::create_dir_all(self.paths.devices_dir())?;
        fs::create_dir_all(self.paths.backups_dir())?;

        let path = self.paths.device_file(&config.name);
        let backup = if path.exists() {
            Some(self.backup_file(&path, &config.name, "edit")?)
        } else {
            None
        };

        let text = config.to_toml().map_err(|error| StoreError::Invalid {
            message: format!("无法序列化配置：{error}"),
            issues: Vec::new(),
        })?;

        atomic_write(&path, text.as_bytes(), 0o644)?;

        Ok(SaveOutcome {
            config: config.clone(),
            path,
            backup,
            warnings,
        })
    }

    pub fn delete(&self, name: &str) -> Result<Option<PathBuf>, StoreError> {
        let path = self.paths.device_file(name);
        if !path.exists() {
            return Err(StoreError::NotFound(name.to_owned()));
        }
        fs::create_dir_all(self.paths.backups_dir())?;
        let backup = self.backup_file(&path, name, "deleted")?;
        fs::remove_file(&path)?;
        Ok(Some(backup))
    }

    fn backup_file(
        &self,
        path: &Path,
        name: &str,
        suffix: &str,
    ) -> Result<PathBuf, std::io::Error> {
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
        let backup = self
            .paths
            .backups_dir()
            .join(format!("{name}-{suffix}-{stamp}.toml"));
        fs::copy(path, &backup)?;
        self.prune_backups(name, 20);
        Ok(backup)
    }

    fn prune_backups(&self, name: &str, keep: usize) {
        let dir = self.paths.backups_dir();
        let mut files: Vec<PathBuf> = match fs::read_dir(&dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|file| file.to_str())
                        .is_some_and(|file| file.starts_with(&format!("{name}-")))
                })
                .collect(),
            Err(_) => return,
        };
        files.sort();
        while files.len() > keep {
            let oldest = files.remove(0);
            let _ = fs::remove_file(oldest);
        }
    }
}

/// Write a file so a reader sees either the old content or the new content,
/// never a truncated record.  `mode` is only applied on Unix.
pub fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let stamp = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let tmp = parent.join(format!(
        ".{}.tmp.{}.{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("devguard"),
        std::process::id(),
        stamp
    ));
    let result = (|| {
        let mut file = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
