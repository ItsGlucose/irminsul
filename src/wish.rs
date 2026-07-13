use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use tokio::sync::watch;

pub struct Wish {
    url_tx: watch::Sender<Option<String>>,
    first_packet_rx: Option<tokio::sync::oneshot::Receiver<()>>,
}

impl Wish {
    pub async fn new(
        url_tx: watch::Sender<Option<String>>,
        first_packet_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<Self> {
        Ok(Self {
            url_tx,
            first_packet_rx: Some(first_packet_rx),
        })
    }

    pub async fn monitor(&mut self) -> Result<()> {
        tracing::info!("Wish monitoring waiting for first packet capture...");
        let Some(rx) = self.first_packet_rx.take() else {
            tracing::warn!("Wish monitor already started or no receiver");
            return Ok(());
        };
        let _ = rx.await;
        tracing::info!("First packet captured, attempting to extract wish URL from cache");

        match extract_wish_url_from_cache().await {
            Ok(url) => {
                tracing::info!("Successfully extracted wish URL");
                let _ = self.url_tx.send(Some(url));
            }
            Err(e) => {
                tracing::error!("Failed to extract wish URL: {e}");
            }
        }

        Ok(())
    }
}

async fn extract_wish_url_from_cache() -> Result<String> {
    // Find game process to get install directory
    let game_dir = find_game_directory()?;
    
    let web_cache_base = game_dir.join("GenshinImpact_Data/webCaches");
    if !web_cache_base.exists() {
        return Err(anyhow!("webCaches directory not found at {}", web_cache_base.display()));
    }

    // Find newest cache folder (version numbered like 2.52.0.0)
    let mut newest_version: Option<(String, PathBuf)> = None;
    
    for entry in std::fs::read_dir(&web_cache_base)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        
        // Check if it's a version folder (all chars are digits or dots)
        if name_str.chars().all(|c| c.is_ascii_digit() || c == '.') {
            if newest_version.is_none() || Some(name.clone()) > newest_version.as_ref().map(|(_, p)| p.file_name().unwrap().to_owned()) {
                newest_version = Some((name_str.to_string(), path));
            }
        }
    }

    let (version, cache_dir) = newest_version
        .ok_or_else(|| anyhow!("No version folders found in webCaches"))?;
    
    tracing::info!("Using cache version: {version}");
    
    let cache_file = cache_dir.join("Cache/Cache_Data/data_2");
    if !cache_file.exists() {
        return Err(anyhow!("Cache data file not found: {}", cache_file.display()));
    }

    // Read cache file and search for wish URL
    let cache_data = std::fs::read(&cache_file)
        .context("Failed to read cache file")?;
    
    let cache_str = String::from_utf8_lossy(&cache_data);
    
    // Find line containing wish gacha URL
    for line in cache_str.lines().rev() {
        if line.contains("e20190909gacha") && line.contains("/index.html") {
            // Extract URL between https:// and null terminators
            if let Some(start) = line.find("https://") {
                let remaining = &line[start..];
                // Find end (either null bytes or end of line)
                let end = remaining.find("\0\0\0\0")
                    .or_else(|| remaining.find('\0'))
                    .unwrap_or(remaining.len());
                
                let url = &remaining[..end];
                
                // Validate it's a proper wish URL
                if url.contains("authkey") && url.contains("hoyoverse.com") {
                    tracing::info!("Found wish URL in cache");
                    return Ok(url.to_string());
                }
            }
        }
    }
    
    Err(anyhow!("No wish URL found in cache - open wish history in-game first"))
}

fn find_game_directory() -> Result<PathBuf> {
    // Scan /proc for GenshinImpact process
    for entry in std::fs::read_dir("/proc")?.filter_map(Result::ok) {
        let Ok(pid_str) = entry.file_name().into_string() else { continue };
        let Ok(_pid) = pid_str.parse::<u32>() else { continue };
        
        let comm_path = format!("/proc/{}/comm", pid_str);
        let Ok(comm) = std::fs::read_to_string(&comm_path) else { continue };
        
        if comm.trim().contains("GenshinImpact") {
            // Get working directory of the process
            let cwd_link = format!("/proc/{}/cwd", pid_str);
            if let Ok(cwd) = std::fs::read_link(&cwd_link) {
                tracing::info!("Found game at: {}", cwd.display());
                // CWD is usually "<install>/Genshin Impact"
                // We need the parent for GenshinImpact_Data
                return Ok(cwd);
            }
        }
    }
    
    Err(anyhow!("GenshinImpact process not found - is the game running?"))
}
