use std::path::{PathBuf, Path};
use std::str;
use std::fmt::Debug;

use serde::ser::{Serialize, Serializer, SerializeStruct};

#[cfg_attr(test, derive(Debug, PartialEq))]
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
    /// Relative path of the thumbnail file, from the library path.
    pub thumbnail_path: Option<PathBuf>,
}


impl Video
{
    pub fn new<P: AsRef<Path> + Debug>(id: String, path: P) -> Self
    {
        Self {
            id,
            path: path.as_ref().to_owned(),
            title: String::new(),
            desc: String::new(),
            artist: String::new(),
            views: 0,
            upload_time: time::OffsetDateTime::UNIX_EPOCH,
            container_type: ContainerType::Mp4,
            original_filename: String::new(),
            duration: time::Duration::default(),
            thumbnail_path: None,
        }
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
        let mut state = serializer.serialize_struct("Video", 11)?;
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
        let hours = self.duration.whole_hours();
        let minutes = (self.duration - time::Duration::hours(hours))
            .whole_minutes();
        let seconds = (self.duration - time::Duration::hours(hours) -
                       time::Duration::minutes(minutes)).whole_seconds();
        let duration_str = if hours > 0
        {
            format!("{}:{:02}:{:02}", hours, minutes, seconds)
        }
        else
        {
            format!("{:02}:{:02}", minutes, seconds)
        };
        state.serialize_field("duration_str", &duration_str)?;
        state.serialize_field(
            "thumbnail_path",
            &self.thumbnail_path.as_ref().map(|p| p.to_str().unwrap()))?;
        state.end()
    }
}
