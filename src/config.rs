use serde::Deserialize;

use crate::error::Error;

fn defaultListenAddr() -> String
{
    String::from("127.0.0.1")
}

fn defaultServePath() -> String
{
    String::from("/")
}

fn defaultListenPort() -> u16 { 8080 }

fn defaultUploadSizeMax() -> u64 { 10 * 1024 * 1024 * 1024 }

#[derive(Deserialize, Clone)]
pub struct Configuration
{
    pub video_dir: String,
    pub static_dir: String,
    pub data_dir: String,
    #[serde(default = "defaultListenAddr")]
    pub listen_address: String,
    #[serde(default = "defaultListenPort")]
    pub listen_port: u16,
    #[serde(default = "defaultServePath")]
    pub serve_under_path: String,
    #[serde(default = "defaultUploadSizeMax")]
    pub upload_size_max: u64,
}

impl Configuration
{
    pub fn fromFile(path: &str) -> Result<Self, Error>
    {
        let content = std::fs::read_to_string(path).map_err(
            |_| rterr!("Failed to read config file at {}", path))?;
        toml::from_str(&content).map_err(
            |_| rterr!("Failed to parse config file"))
    }
}

impl Default for Configuration
{
    fn default() -> Self
    {
        Self {
            video_dir: String::from("."),
            static_dir: String::from("static"),
            data_dir: String::from("."),
            listen_address: defaultListenAddr(),
            listen_port: defaultListenPort(),
            serve_under_path: defaultServePath(),
            upload_size_max: defaultUploadSizeMax(),
        }
    }
}
