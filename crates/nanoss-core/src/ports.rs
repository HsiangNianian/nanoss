use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::Method;

pub trait HttpPort: Send + Sync {
    fn request_status(&self, method: Method, url: &str, timeout_secs: u64, user_agent: &str) -> Option<u16>;
    fn get_json(&self, url: &str, timeout_secs: u64, user_agent: &str) -> Result<serde_json::Value>;
}

pub trait FileSystemPort: Send + Sync {
    fn read_to_string(&self, path: &Path) -> Result<String>;
    fn write_string(&self, path: &Path, content: &str) -> Result<()>;
    fn create_dir_all(&self, path: &Path) -> Result<()>;
    fn exists(&self, path: &Path) -> bool;
}

pub trait ProcessPort: Send + Sync {
    fn run(&self, binary: &str, args: &[String], timeout_secs: u64) -> Result<()>;
}

pub struct StdFileSystemPort;

impl FileSystemPort for StdFileSystemPort {
    fn read_to_string(&self, path: &Path) -> Result<String> {
        std::fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
    }

    fn write_string(&self, path: &Path, content: &str) -> Result<()> {
        std::fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        std::fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

pub struct StdHttpPort;

impl HttpPort for StdHttpPort {
    fn request_status(&self, method: Method, url: &str, timeout_secs: u64, user_agent: &str) -> Option<u16> {
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .user_agent(user_agent)
            .build()
            .ok()?;
        client
            .request(method, url)
            .send()
            .ok()
            .map(|response| response.status().as_u16())
    }

    fn get_json(&self, url: &str, timeout_secs: u64, user_agent: &str) -> Result<serde_json::Value> {
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .user_agent(user_agent)
            .build()
            .context("failed to create HTTP client")?;
        let response = client
            .get(url)
            .send()
            .with_context(|| format!("failed to send GET request to {url}"))?
            .error_for_status()
            .with_context(|| format!("non-success status while requesting {url}"))?;
        let body = response
            .text()
            .with_context(|| format!("failed to read response body from {url}"))?;
        serde_json::from_str(&body).with_context(|| format!("failed to decode JSON from {url}"))
    }
}

pub struct StdProcessPort;

impl ProcessPort for StdProcessPort {
    fn run(&self, binary: &str, args: &[String], timeout_secs: u64) -> Result<()> {
        crate::ensure_binary_name_safe(binary)?;
        let mut cmd = Command::new(binary);
        for arg in args {
            cmd.arg(arg);
        }
        let mut child = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to execute {binary}"))?;
        let status = crate::wait_child_with_timeout(&mut child, timeout_secs)
            .with_context(|| format!("{binary} timed out"))?;
        if !status.success() {
            bail!("{binary} failed");
        }
        Ok(())
    }
}

