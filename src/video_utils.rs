use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::video::Video;
use crate::config::Configuration;

pub fn videoPath(video: &Video, config: &Configuration) -> PathBuf
{
    Path::new(&config.video_dir).join(&video.path)
}

pub fn expectedThumbnailPath(video: &Video, config: &Configuration) -> PathBuf
{
    Path::new(&config.video_dir).join(&video.path).with_extension("webp")
}

pub fn findThumbnail(video: &Video, config: &Configuration) -> Option<PathBuf>
{
    let path = expectedThumbnailPath(video, config);
    if path.exists()
    {
        Some(path)
    }
    else
    {
        None
    }
}
