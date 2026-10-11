//! The servers the start window offers next to this Mac, saved in the app's
//! data directory. A server is an ssh destination: a `Host` from
//! `~/.ssh/config`, or `user@host`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use sandhill_link::Target;
use tauri::State;

pub struct Servers {
    saved: PathBuf,
    list: Mutex<Vec<String>>,
}

impl Servers {
    pub fn load(saved: PathBuf) -> Self {
        let list = std::fs::read_to_string(&saved)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Servers {
            saved,
            list: Mutex::new(list),
        }
    }

    pub fn list(&self) -> Vec<String> {
        self.list.lock().unwrap().clone()
    }

    pub fn add(&self, host: &str) -> Result<Vec<String>, String> {
        let host = host.trim();
        match Target::parse(host).map_err(|err| err.to_string())? {
            Target::Local => return Err("\"local\" is this Mac".into()),
            Target::Ssh(_) => {}
        }
        let mut list = self.list.lock().unwrap();
        if !list.iter().any(|h| h == host) {
            list.push(host.to_string());
            self.save(&list)?;
        }
        Ok(list.clone())
    }

    pub fn remove(&self, host: &str) -> Result<Vec<String>, String> {
        let mut list = self.list.lock().unwrap();
        list.retain(|h| h != host);
        self.save(&list)?;
        Ok(list.clone())
    }

    fn save(&self, list: &[String]) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(list).map_err(|err| err.to_string())?;
        if let Some(dir) = self.saved.parent() {
            std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
        }
        std::fs::write(&self.saved, bytes)
            .map_err(|err| format!("cannot save the list of servers: {err}"))
    }
}

#[tauri::command]
pub fn list_servers(servers: State<'_, Arc<Servers>>) -> Vec<String> {
    servers.list()
}

/// Adds a server to the list and returns the list.
#[tauri::command]
pub fn add_server(servers: State<'_, Arc<Servers>>, host: String) -> Result<Vec<String>, String> {
    servers.add(&host)
}

/// Removes a server from the list (nothing on the server changes).
#[tauri::command]
pub fn remove_server(
    servers: State<'_, Arc<Servers>>,
    host: String,
) -> Result<Vec<String>, String> {
    servers.remove(&host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_are_remembered_once_each() {
        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("servers.json");
        let servers = Servers::load(saved.clone());
        assert!(servers.list().is_empty());
        servers.add("lab").unwrap();
        servers.add(" huan@10.0.0.5 ").unwrap();
        assert_eq!(servers.add("lab").unwrap(), ["lab", "huan@10.0.0.5"]);
        assert!(servers.add("local").is_err());
        assert!(servers.add("-oProxyCommand=x").is_err());

        let again = Servers::load(saved);
        assert_eq!(again.list(), ["lab", "huan@10.0.0.5"]);
        assert_eq!(again.remove("lab").unwrap(), ["huan@10.0.0.5"]);
    }
}
