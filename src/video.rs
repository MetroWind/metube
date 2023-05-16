use std::path::{PathBuf, Path};
use std::process::Command;
use std::collections::HashMap;
use std::str;
use std::fmt::Debug;

use serde::ser::{Serialize, Serializer, SerializeStruct};
use regex::Regex;
use log::debug;

use crate::error::Error;
use crate::config::Configuration;

pub enum ContainerType
{
    Mp4, WebM
}

impl ContainerType
{
    pub fn fromExtension(ext: &str) -> Option<Self>
    {
        match ext.to_ascii_lowercase().as_str()
        {
            "mp4" => Some(Self::Mp4),
            "webm" => Some(Self::WebM),
            _ => None,
        }
    }

    pub fn fromFormatName(name: &str) -> Option<Self>
    {
        match name
        {
            "mov,mp4,m4a,3gp,3g2,mj2" => Some(Self::Mp4),
            "matroska,webm" => Some(Self::WebM),
            _ => None,
        }
    }

    pub fn toExtension(&self) -> &str
    {
        match self
        {
            Self::Mp4 => "mp4",
            Self::WebM => "webm",
        }
    }

    pub fn contentType(&self) -> &str
    {
        match self
        {
            Self::Mp4 => "video/mp4",
            Self::WebM => "video/webm",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProbedMetadataSection
{
    pub name: String,
    pub metadata: HashMap<String, String>,
}

impl ProbedMetadataSection
{
    pub fn new() -> Self
    {
        Self { name: String::new(), metadata: HashMap::new() }
    }
}

fn parseProbeOutput(output: &str) -> Result<Vec<ProbedMetadataSection>, Error>
{
    let sec_begin_pattern = Regex::new(r"^\[([^/]+)\]$").unwrap();
    let sec_end_pattern = Regex::new(r"^\[/([^/]+)\]$").unwrap();
    let mut result = Vec::new();
    let mut current_section = ProbedMetadataSection::new();
    for line in output.lines()
    {
        if line.is_empty()
        {
            continue;
        }
        if let Some(cap) = sec_begin_pattern.captures(line)
        {
            current_section = ProbedMetadataSection::new();
            current_section.name = cap.get(1).unwrap().as_str().to_owned();
        }
        else if let Some(cap) = sec_end_pattern.captures(line)
        {
            if cap.get(1).unwrap().as_str() != current_section.name
            {
                return Err(rterr!("Unmatched section end: expect {}, found {}.",
                                  current_section.name,
                                  cap.get(1).unwrap().as_str()));
            }
            result.push(current_section.clone());
        }
        else
        {
            let mut split = line.splitn(2, "=");
            let key = split.next().ok_or_else(
                || rterr!("Invalid metadata line: {}", line))?;
            let value = split.next().ok_or_else(
                || rterr!("Invalid metadata line: {}", line))?;
            current_section.metadata.insert(key.to_owned(), value.to_owned());
        }
    }
    debug!("Metadata from probe: {:?}", result);
    Ok(result)
}

pub struct Video
{
    pub id: String,
    /// Relative path of the video, from the library path.
    pub path: PathBuf,
    pub title: String,
    pub desc: String,
    pub artist: String,
    pub views: u32,
    /// This should always be in UTC.
    pub upload_time: time::OffsetDateTime,
    pub container_type: ContainerType,
    /// The original filename from user upload. May be empty.
    pub original_filename: String,
    pub duration: time::Duration,
}

fn probeVideo(f: &Path) -> Result<Vec<ProbedMetadataSection>, Error>
{
    let output = Command::new("ffprobe").arg("-show_format")
        .arg(f.to_str().ok_or_else(|| rterr!("Invalid video path: {:?}", f))?)
        .output().map_err(|e| rterr!("Failed to run ffprobe: {}", e))?;
    if !output.status.success()
    {
        if let Some(code) = output.status.code()
        {
            return Err(rterr!("Ffprobe failed with code {}.", code));
        }
        else
        {
            return Err(rterr!("Ffprobe terminated with signal."));
        }
    }
    parseProbeOutput(unsafe { str::from_utf8_unchecked(&output.stdout) })
}

fn fillProbedMetadata(mut video: Video, metadata: Vec<ProbedMetadataSection>) ->
    Result<Video, Error>
{
    for section in metadata
    {
        if section.name == "FORMAT"
        {
            if let Some(value) = section.metadata.get("format_name")
            {
                video.container_type = ContainerType::fromFormatName(value)
                    .ok_or_else(|| rterr!("Unsupported format_name: {}",
                                          value))?;
            }
            else
            {
                return Err(rterr!("format_name not found"));
            }
            if let Some(value) = section.metadata.get("TAG:title")
            {
                video.title = value.clone();
            }
            else if let Some(value) = section.metadata.get("TAG:comment")
            {
                video.desc = value.clone();
            }
            else if let Some(value) = section.metadata.get("TAG:artist")
            {
                video.artist = value.clone();
            }
            else if let Some(value) = section.metadata.get("duration")
            {
                debug!("Duration string is {}.", value);
                video.duration = time::Duration::seconds_f64(
                    value.parse().map_err(
                        |_| rterr!("Invalid duration: {}", value))?);
            }
        }
    }
    Ok(video)
}

impl Video
{
    // TODO: move this to app.rs.
    pub fn fromFile<P: AsRef<Path> + Debug>(f: P, video_dir: &str) ->
        Result<Self, Error>
    {
        let p: &Path = f.as_ref();
        debug!("Processing {}...", p.display());
        let full_path = p.canonicalize().map_err(
            |e| rterr!("Failed to canonicalize path {:?}: {}", p, e))?;
        let video_dir = Path::new(video_dir).canonicalize().map_err(
            |e| rterr!("Failed to canonicalize path {:?}: {}", video_dir, e))?;
        if !full_path.exists()
        {
            return Err(rterr!("Video not found: {:?}", f));
        }
        let path = full_path.strip_prefix(video_dir).map_err(
            |_| rterr!("Video is not in the video directory."))?;

        let metadata = probeVideo(&full_path)?;
        let video = Self {
            id: full_path.with_extension("").file_name().ok_or_else(
                || rterr!("Invalid video path without file name"))?.to_str()
                .ok_or_else(|| rterr!("Invalid video path"))?.to_owned(),
            path: path.to_owned(),
            title: String::new(),
            desc: String::new(),
            artist: String::new(),
            views: 0,
            upload_time: time::OffsetDateTime::UNIX_EPOCH,
            container_type: ContainerType::Mp4,
            original_filename: p.file_name().ok_or_else(
                || rterr!("Invalid video path without file name"))?.to_str()
                .ok_or_else(|| rterr!("Invalid video path"))?.to_owned(),
            duration: time::Duration::default(),
        };
        fillProbedMetadata(video, metadata)
    }

    pub fn category(&self) -> &Path
    {
        self.path.parent().unwrap()
    }

    pub fn displayTitle(&self) -> &str
    {
        if self.title.is_empty()
        {
            &self.original_filename
        }
        else
        {
            &self.title
        }
    }
}

impl Serialize for Video
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // 3 is the number of fields in the struct.
        let mut state = serializer.serialize_struct("Video", 10)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field(
            "path", &self.path.to_str().ok_or_else(
                || serde::ser::Error::custom("Invalid path"))?)?;
        state.serialize_field("title", self.displayTitle())?;
        state.serialize_field("desc", &self.desc)?;
        state.serialize_field("artist", &self.artist)?;
        state.serialize_field("views", &self.views)?;
        state.serialize_field("upload_time",
                              &self.upload_time.unix_timestamp())?;
        let format: Vec<time::format_description::FormatItem> =
            time::format_description::parse(
                "[year]-[month]-[day] [hour]:[minute]:[second] UTC").unwrap();
        state.serialize_field(
            "upload_time_utc_str", &self.upload_time.format(&format).map_err(
                |_| serde::ser::Error::custom("Invalid upload time"))?)?;
        state.serialize_field(
            "container_type", &self.container_type.toExtension())?;
        state.serialize_field(
            "content_type", &self.container_type.contentType())?;
        state.serialize_field(
            "duration_sec", &self.duration.as_seconds_f64())?;
        state.end()
    }
}
