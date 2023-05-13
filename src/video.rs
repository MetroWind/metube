use std::path::{PathBuf, Path};
use std::process::Command;
use std::collections::HashMap;
use std::str;
use std::fmt::Debug;

use serde::ser::{Serialize, Serializer, SerializeStruct};
use regex::Regex;

use crate::error::Error;

pub enum Type
{
    Mp3, Mp4, WebP,
}

impl Type
{
    pub fn fromExtension(ext: &str) -> Option<Self>
    {
        match ext.to_ascii_lowercase().as_str()
        {
            "mp3" => Some(Self::Mp3),
            "mp4" => Some(Self::Mp4),
            "webp" => Some(Self::WebP),
            _ => None,
        }
    }
}

#[derive(Clone)]
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
    Video
{
    for section in metadata
    {
        if section.name == "FORMAT"
        {
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
        }
    }
    video
}

impl Video
{
    pub fn fromFile<P: AsRef<Path> + Debug>(f: P, video_dir: &str) ->
        Result<Self, Error>
    {
        let p: &Path = f.as_ref();
        let full_path = p.canonicalize().map_err(
            |e| rterr!("Failed to canonicalize path {:?}: {}", p, e))?;
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
        };
        Ok(fillProbedMetadata(video, metadata))
    }

    pub fn category(&self) -> &Path
    {
        self.path.parent().unwrap()
    }
}

impl Serialize for Video
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // 3 is the number of fields in the struct.
        let mut state = serializer.serialize_struct("Video", 8)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field(
            "path", &self.path.to_str().ok_or_else(
                || serde::ser::Error::custom("Invalid path"))?)?;
        state.serialize_field("title", &self.title)?;
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
        state.end()
    }
}
