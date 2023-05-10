use std::path::{PathBuf, Path};

use crate::error::Error;

pub struct Video
{
    /// Full path of the video
    pub path: PathBuf,
    /// Full path of the thumbnail file
    pub thumbnail: Option<PathBuf>,
    pub title: String,
    pub desc: String,
}

impl Video
{
    /// Path `f` is a path accessible from the CWD.
    pub fn fromFile(f: &Path) -> Result<Self, Error>
    {
        if !f.exists()
        {
            return Err(rterr!("Video file not found: {:?}", f));
        }
        Ok(Self {
            path: f.canonicalize().map_err(
                |e| rterr!("Failed to canonicalize path {:?}: {}", f, e))?,
            thumbnail: None,
            title: String::new(),
            desc: String::new(),
        })
    }
}
