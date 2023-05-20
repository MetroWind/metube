use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::io::prelude::*;
use std::io::BufWriter;
use std::fs::File;
use std::ffi::OsStr;
use std::process::Command;
use std::str;

use futures_util::StreamExt;
use bytes::buf::Buf;
use log::debug;
use log::error as log_error;
use time::OffsetDateTime;
use warp::http::status::StatusCode;
use sha2::Digest;
use regex::Regex;

use crate::data;
use crate::error::Error;
use crate::video::{Video, ContainerType};
use crate::config::Configuration;

pub fn videoPath(video: &Video, config: &Configuration) -> PathBuf
{
    Path::new(&config.video_dir).join(&video.path)
}

pub fn expectedThumbnailPath(video: &Video, config: &Configuration) -> PathBuf
{
    Path::new(&config.video_dir).join(&video.path).with_extension("webp")
}

fn randomTempFilename<P: AsRef<Path>>(dir: P) -> PathBuf
{
    loop
    {
        let filename = format!("temp-{}", rand::random::<u32>());
        let path = dir.as_ref().join(&filename);
        if !path.exists()
        {
            return path;
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

            if let Some(value) = section.metadata.get("duration")
            {
                video.duration = time::Duration::seconds_f64(
                    value.parse().map_err(
                        |_| rterr!("Invalid duration: {}", value))?);
            }
            else
            {
                return Err(rterr!("Duration not found"));
            }

            // Get title from possible tags.
            if let Some(value) = section.metadata.get("TAG:title")
            {
                video.title = value.clone();
            }
            else if let Some(value) = section.metadata.get("TAG:TITLE")
            {
                video.title = value.clone();
            }

            // Get comment from possible tags.
            if let Some(value) = section.metadata.get("TAG:comment")
            {
                video.desc = value.clone();
            }
            else if let Some(value) = section.metadata.get("TAG:COMMENT")
            {
                video.desc = value.clone();
            }

            // Get artist from possible tags.
            if let Some(value) = section.metadata.get("TAG:artist")
            {
                video.artist = value.clone();
            }
            else if let Some(value) = section.metadata.get("TAG:author")
            {
                video.artist = value.clone();
            }
            else if let Some(value) = section.metadata.get("TAG:ARTIST")
            {
                video.artist = value.clone();
            }
            else if let Some(value) = section.metadata.get("TAG:AUTHOR")
            {
                video.artist = value.clone();
            }
        }
    }
    Ok(video)
}

/// Some bytes that are being uploaded
pub struct UploadingVideo
{
    pub part: warp::multipart::Part,
}

/// A video file that is just uploaded.
pub struct RawVideo
{
    /// Path of the video file, accessible from the CWD.
    pub path: PathBuf,
    pub hash: String,
    pub original_filename: String,
}

impl UploadingVideo
{
    /// This will create a temp file under the video directory. This
    /// is important, because later the video will be renamed to the
    /// correct name. We need the rename to happen in the same storage
    /// volumn so that it can succeed.
    pub async fn saveToTemp(self, config: &Configuration) ->
        Result<RawVideo, Error>
    {
        let orig_name = self.part.filename().map(|n| n.to_owned()).ok_or_else(
            || Error::HTTPStatus(StatusCode::BAD_REQUEST,
                                 String::from("No filename in upload")))?;
        let temp_file = randomTempFilename(&config.video_dir)
            .with_extension(Path::new(&orig_name).extension()
                            .or(Some(OsStr::new(""))).unwrap());
        let mut f = match File::create(&temp_file)
        {
            Ok(f) => BufWriter::new(f),
            Err(e) => {
                return Err(rterr!("Failed to open temp file: {}", e));
            },
        };
        let mut hasher = sha2::Sha256::new();
        let mut buffers = self.part.stream();
        while let Some(buffer) = buffers.next().await
        {
            if buffer.is_err()
            {
                if std::fs::remove_file(&temp_file).is_err()
                {
                    log_error!("Failed to remove temp file at {:?}.", temp_file);
                }
            }
            let mut buffer = buffer.map_err(
                |e| rterr!("Failed to acquire buffer from form data: {}", e))?;
            while buffer.has_remaining()
            {
                let bytes = buffer.chunk();
                hasher.update(bytes);
                if let Err(e) = f.write_all(bytes)
                {
                    drop(f);
                    if std::fs::remove_file(&temp_file).is_err()
                    {
                        log_error!("Failed to remove temp file at {:?}.", temp_file);
                    }
                    return Err(rterr!("Failed to write temp file: {}", e));
                }
                buffer.advance(bytes.len());
            }
        }

        let hash = hasher.finalize();
        let byte_strs: Vec<_> = hash[..6].iter().map(|b| format!("{:02x}", b))
            .collect();

        Ok(RawVideo {
            path: temp_file,
            hash: byte_strs.join(""),
            original_filename: orig_name,
        })
    }
}

impl RawVideo
{
    pub fn moveToLibrary(self, config: &Configuration) ->
        Result<Self, Error>
    {
        let ext = self.path.extension().or(Some(OsStr::new(""))).unwrap();
        let video_file: PathBuf = Path::new(&config.video_dir).join(&self.hash)
            .with_extension(ext);
        debug!("Moving video {:?} --> {:?}...", self.path, video_file);
        if let Err(e) = std::fs::rename(&self.path, &video_file)
        {
            std::fs::remove_file(&self.path).ok();
            std::fs::remove_file(&video_file).ok();
            return Err(rterr!("Failed to rename temp file: {}", e));
        }
        Ok(Self {
            path: video_file,
            hash: self.hash,
            original_filename: self.original_filename
        })
    }

    pub fn makeRelativePath(mut self, config: &Configuration) ->
        Result<Self, Error>
    {
        let full_path = self.path.canonicalize().map_err(
            |e| {
                std::fs::remove_file(&self.path).ok();
                rterr!("Failed to canonicalize path {:?}: {}", self.path, e)
            })?;
        let video_dir = Path::new(&config.video_dir).canonicalize().map_err(
            |e| {
                std::fs::remove_file(&self.path).ok();
                rterr!("Failed to canonicalize path {:?}: {}",
                       config.video_dir, e)
            })?;
        if !full_path.exists()
        {
            std::fs::remove_file(&self.path).ok();
            return Err(rterr!("Video not found: {:?}", full_path));
        }
        let path = full_path.strip_prefix(video_dir).map_err(
            |_| {
                std::fs::remove_file(&full_path).ok();
                rterr!("Video is not in the video directory.")
            })?;
        self.path = path.to_owned();
        Ok(self)
    }

    pub fn probeMetadata(self, config: &Configuration) -> Result<Video, Error>
    {
        let mut video = Video::new(self.hash, &self.path);
        video.original_filename = self.original_filename;
        video.upload_time = OffsetDateTime::now_utc();
        let metadata = match probeVideo(
            &Path::new(&config.video_dir).join(&self.path))
        {
            Ok(data) => data,
            Err(e) => {
                std::fs::remove_file(&Path::new(&config.video_dir)
                                     .join(&self.path)).ok();
                return Err(e);
            },
        };

        match fillProbedMetadata(video, metadata)
        {
            Ok(video) => Ok(video),
            Err(e) => {
                std::fs::remove_file(
                    &Path::new(&config.video_dir).join(&self.path)).ok();
                Err(e)
            }
        }
    }
}
impl Video
{
    /// Thumbnail generation shouldn’t usually fail. This function
    /// should almost always return Ok(), unless something panicking
    /// happend.
    pub fn generateThumbnail(mut self, config: &Configuration) ->
        Result<Video, Error>
    {
        let thumb_time_sec = if self.duration > time::Duration::seconds(30)
        {
            10.0
        }
        else
        {
            self.duration.as_seconds_f64() / 3.0
        };
        let video_path = videoPath(&self, config);
        let thumbnail_path = expectedThumbnailPath(&self, config);
        let status = Command::new("ffmpeg")
            .args(["-y", "-i", video_path.to_str().unwrap(), "-ss",
                   &thumb_time_sec.to_string(), "-frames:v", "1", "-vf",
                   r#"scale=if(gte(iw\,ih)\,min(512\,iw)\,-2):if(lt(iw\,ih)\,min(512\,ih)\,-2)"#,
                   "-c:v", "libwebp", "-q:v",
                   &config.thumbnail_quality.to_string(),
                   thumbnail_path.to_str().unwrap()])
            .stderr(std::process::Stdio::null())
            .status();
        if status.is_err()
        {
            return Ok(self);
        }
        if status.unwrap().success()
        {
            self.thumbnail_path = Some(self.path.with_extension("webp"));
        }
        Ok(self)
    }

    pub fn addToDatabase(self, config: &Configuration,
                         data_manager: &data::Manager) -> Result<(), Error>
    {
        if let Err(e) = data_manager.addVideo(&self)
        {
            std::fs::remove_file(&videoPath(&self, config)).ok();
            return Err(e)
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests
{
    use super::*;

    struct FileDeleter
    {
        files: Vec<PathBuf>,
    }

    impl FileDeleter
    {
        fn new() -> Self
        {
            Self { files: Vec::new() }
        }

        fn register<P: AsRef<Path>>(&mut self, f: P)
        {
            let p: &Path = f.as_ref();
            self.files.push(p.to_owned());
        }
    }

    impl Drop for FileDeleter
    {
        fn drop(&mut self)
        {
            for f in &self.files
            {
                std::fs::remove_file(&f).ok();
            }
        }
    }

    #[test]
    fn testVideoPipeline() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut clean_up = FileDeleter::new();
        let video_dir = std::env::temp_dir();
        let mut config = Configuration::default();
        config.video_dir = video_dir.to_str().ok_or(
            rterr!("Invalid video dir"))?.to_owned();
        let temp_file = video_dir.join("test.webm");
        std::fs::copy("test-data/test-av1-opus.webm", &temp_file)?;
        clean_up.register(video_dir.join("test.webm"));
        let v = RawVideo {
            path: temp_file,
            hash: "12345".to_owned(),
            original_filename: "test-av1-opus.webm".to_owned(),
        };
        let mut data_manager = data::Manager::new(
            crate::sqlite_connection::Source::Memory);
        data_manager.connect()?;
        data_manager.init()?;
        clean_up.register(video_dir.join("12345.webm"));
        clean_up.register(video_dir.join("12345.webp"));
        v.moveToLibrary(&config)?
            .makeRelativePath(&config)?
            .probeMetadata(&config)?
            .generateThumbnail(&config)?
            .addToDatabase(&config, &data_manager)?;

        let v = data_manager.findVideoByID("12345")?;
        assert!(v.is_some());
        let v = v.unwrap();
        assert_eq!(&v.id, "12345");
        assert_eq!(v.path.to_str().unwrap(), "12345.webm");
        assert_eq!(&v.title, "AV1 + Opus test");
        assert_eq!(&v.artist, "MetroWind");
        assert_eq!(&v.desc, "It's a test");
        assert_eq!(v.views, 0);
        assert!(v.upload_time.unix_timestamp() > 0);
        assert_eq!(v.container_type, ContainerType::WebM);
        assert_eq!(&v.original_filename, "test-av1-opus.webm");
        assert_eq!(v.duration, time::Duration::seconds(10));
        assert!(v.thumbnail_path.is_some());
        assert!(video_dir.join(&v.thumbnail_path.unwrap()).exists());

        Ok(())
    }
}
