use std::path::{PathBuf, Path};

use crate::error::Error;

pub struct Video
{
    pub id: String,
    /// Full path of the video
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
        if !p.exists()
        {
            return Err(rterr!("Video file not found: {:?}", p));
        }
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
}
