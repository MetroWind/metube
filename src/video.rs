use std::path::{PathBuf, Path};

use serde::ser::{Serialize, Serializer, SerializeStruct};

use crate::error::Error;

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

impl Video
{
    /// Path `f` is a path accessible from the CWD.
    pub fn fromFile<P: AsRef<Path>>(f: P) -> Result<Self, Error>
    {
        let p: &Path = f.as_ref();
        Ok(Self {
            id: String::new(),
            path: p.canonicalize().map_err(
                |e| rterr!("Failed to canonicalize path {:?}: {}", p, e))?,
            title: String::new(),
            desc: String::new(),
            artist: String::new(),
            views: 0,
            upload_time: time::OffsetDateTime::UNIX_EPOCH,
        })
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
