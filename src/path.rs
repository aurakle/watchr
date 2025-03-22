use std::path::PathBuf;
use once_cell::sync::Lazy;
use anyhow::Result;
use dirs;

pub struct PathConfig {
    config_dir: PathBuf,
    media_file: PathBuf,
    socket_dir: PathBuf,
}

impl PathConfig {
    pub fn new() -> Result<Self> {
        let home = dirs::home_dir().unwrap();
        let config_dir = home.join(".config").join("watchr");

        std::fs::create_dir_all(&config_dir)?;

        Ok(Self {
            config_dir: config_dir.clone(),
            media_file: config_dir.join("media.mkv"),
            socket_dir: config_dir,
        })
    }

    pub fn socket_path(&self, suffix: &str) -> PathBuf {
        self.socket_dir.join(format!("sock.{}", suffix))
    }

    pub fn media_file(&self) -> PathBuf {
        self.media_file.clone()
    }

    pub fn log_path(&self) -> PathBuf {
        self.config_dir.join("log.mpv")
    }

    pub fn make_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.config_dir)?;
        Ok(())
    }
}

pub static PATHS: Lazy<PathConfig> = Lazy::new(|| {
    PathConfig::new().unwrap()
});