use serde::{Deserialize, Serialize};

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
fn defaultPassword() -> String { "metube".to_owned() }
fn defaultSessionLifeTime() -> u64 {
    time::Duration::days(30).as_seconds_f64() as u64
}
fn defaultThumbnailQuality() -> u8 { 85 }
fn defaultSiteTitle() -> String { String::from("MeTube") }
fn defaultFootnote() -> String { String::new() }
fn defaultUrlDomain() -> String { String::from("http://example.org") }

#[derive(Deserialize, Serialize, Clone)]
pub struct SiteInfo
{
    #[serde(default = "defaultSiteTitle")]
    pub site_title: String,
    #[serde(default = "defaultFootnote")]
    pub footnote: String,
    /// The beginning part of the URL of the website, including only
    /// the protocol and domain, without the trailing slash. This is
    /// only used in the OGP metadata. Example: http://example.org.
    #[serde(default = "defaultUrlDomain")]
    pub url_domain: String,
}

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
    /// Must starts with `/`, and does not end with `/`, unless it’s
    /// just `/`.
    #[serde(default = "defaultServePath")]
    pub serve_under_path: String,
    #[serde(default = "defaultUploadSizeMax")]
    pub upload_size_max: u64,
    #[serde(default = "defaultPassword")]
    pub password: String,
    #[serde(default = "defaultSessionLifeTime")]
    pub session_life_time_sec: u64,
    /// Default compression quality of the WebP thumbnail images,
    /// ranging from 0 to 100. Higher is better. This is passed to
    /// ffmpeg’s `-q:v` argument.
    #[serde(default = "defaultThumbnailQuality")]
    pub thumbnail_quality: u8,
    pub site_info: SiteInfo,
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

impl Default for SiteInfo
{
    fn default() -> Self
    {
        Self {
            site_title: defaultSiteTitle(),
            footnote: defaultFootnote(),
            url_domain: defaultUrlDomain(),
        }
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
            password: defaultPassword(),
            session_life_time_sec: defaultSessionLifeTime(),
            thumbnail_quality: defaultThumbnailQuality(),
            site_info: SiteInfo::default(),
        }
    }
}
