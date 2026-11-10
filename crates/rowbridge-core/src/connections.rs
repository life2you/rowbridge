//! 数据源配置：保存在 `~/.RowBridge/connections.json`。
//!
//! 用可读的 JSON 而不是数据库，方便用户直接查看、备份、手工编辑，
//! 同时也方便 CLI 和 AI Agent 复用已配置的数据源。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config::ConnConfig;

/// 一个已保存的数据源。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedConnection {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// 明文保存在 connections.json。文件权限 700。
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub use_tls: bool,
    /// 这个数据源最近一次用的库，重新选中时自动带上
    #[serde(default)]
    pub last_schema: Option<String>,
}

impl SavedConnection {
    pub fn to_conn_config(&self, schema: Option<String>) -> ConnConfig {
        ConnConfig {
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            password: self.password.clone(),
            schema,
            use_tls: self.use_tls,
        }
    }

    /// 脱敏摘要（供 Agent 或日志查看，不暴露密码）
    pub fn safe_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "host": self.host,
            "port": self.port,
            "username": self.username,
            "use_tls": self.use_tls,
            "last_schema": self.last_schema,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    connections: Vec<SavedConnection>,
}

/// `~/.RowBridge` —— 用户目录下。
pub fn config_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("无法定位用户目录")?;
    Ok(PathBuf::from(home).join(".RowBridge"))
}

fn file_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("connections.json"))
}

fn load() -> Result<File, String> {
    let p = file_path()?;
    if !p.exists() {
        return Ok(File::default());
    }
    let text =
        std::fs::read_to_string(&p).map_err(|e| format!("读取 {} 失败: {e}", p.display()))?;
    Ok(serde_json::from_str(&text).unwrap_or_default())
}

fn save(f: &File) -> Result<(), String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败: {e}", dir.display()))?;
    let p = dir.join("connections.json");
    let text = serde_json::to_string_pretty(f).map_err(|e| e.to_string())?;
    let tmp = dir.join("connections.json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写入失败: {e}"))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("保存失败: {e}"))?;
    Ok(())
}

/// 启动时确保目录存在，并把权限收紧。
pub fn init() -> Result<(), String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

pub fn list() -> Result<Vec<SavedConnection>, String> {
    Ok(load()?.connections)
}

/// 根据 ID 或 名称 查找已保存的数据源
pub fn find(name_or_id: &str) -> Option<SavedConnection> {
    let list = list().ok()?;
    list.into_iter()
        .find(|c| c.id == name_or_id || c.name.eq_ignore_ascii_case(name_or_id))
}

pub fn upsert(c: SavedConnection) -> Result<Vec<SavedConnection>, String> {
    let mut f = load()?;
    match f.connections.iter_mut().find(|x| x.id == c.id) {
        Some(existing) => *existing = c,
        None => f.connections.push(c),
    }
    save(&f)?;
    Ok(f.connections)
}

pub fn remove(id: &str) -> Result<Vec<SavedConnection>, String> {
    let mut f = load()?;
    f.connections.retain(|x| x.id != id);
    save(&f)?;
    Ok(f.connections)
}

pub fn remember_schema(id: &str, schema: &str) -> Result<(), String> {
    let mut f = load()?;
    if let Some(c) = f.connections.iter_mut().find(|x| x.id == id) {
        if c.last_schema.as_deref() != Some(schema) {
            c.last_schema = Some(schema.to_string());
            save(&f)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrips_in_json() {
        let c = SavedConnection {
            id: "t1".into(),
            name: "测试".into(),
            host: "127.0.0.1".into(),
            port: 3307,
            username: "root".into(),
            password: "root123".into(),
            use_tls: false,
            last_schema: None,
        };
        let json = serde_json::to_string_pretty(&c).unwrap();
        assert!(json.contains("root123"), "密码必须在 JSON 里: {json}");
        let back: SavedConnection = serde_json::from_str(&json).unwrap();
        assert_eq!(back.password, "root123");
    }
}
