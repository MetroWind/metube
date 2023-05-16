use std::path::{PathBuf, Path};
use std::collections::HashMap;
use std::io::prelude::*;
use std::io::BufWriter;
use std::fs::File;
use std::ffi::OsStr;
use std::process::Command;

use futures_util::TryStreamExt;
use futures_util::StreamExt;
use bytes::buf::Buf;
use log::{info, debug};
use log::error as log_error;
use tera::Tera;
use warp::{Filter, Reply};
use warp::http::status::StatusCode;
use warp::reply::{Response, Html};
use warp::reject::Reject;
use warp::redirect::AsLocation;
use sha2::Digest;
use base64::engine::Engine;

use crate::error::Error;
use crate::config::Configuration;
use crate::data;
use crate::video::Video;
use crate::video_processing::{expectedThumbnailPath, videoPath};

static BASE64: &base64::engine::general_purpose::GeneralPurpose =
    &base64::engine::general_purpose::STANDARD;
static BASE64_NO_PAD: &base64::engine::general_purpose::GeneralPurpose =
    &base64::engine::general_purpose::STANDARD_NO_PAD;
static TOKEN_COOKIE: &str = "metube-token";

trait ToResponse
{
    fn toResponse(self) -> Response;
}

impl ToResponse for Result<String, Error>
{
    fn toResponse(self) -> Response
    {
        match self
        {
            Ok(s) => warp::reply::html(s).into_response(),
            Err(e) => {
                log_error!("{}", e);
                e.into_response()
            },
        }
    }
}

impl ToResponse for Result<Response, Error>
{
    fn toResponse(self) -> Response
    {
        match self
        {
            Ok(s) => s,
            Err(e) => {
                log_error!("{}", e);
                e.into_response()
            }
        }
    }
}

fn generateThumbnail(video: &Video, config: &Configuration) ->
    Result<PathBuf, Error>
{
    let thumb_time_sec = if video.duration > time::Duration::seconds(30)
    {
        10.0
    }
    else
    {
        video.duration.as_seconds_f64() / 3.0
    };
    let video_path = videoPath(video, config);
    let thumbnail_path = expectedThumbnailPath(video, config);
    let status = Command::new("ffmpeg")
        .args(["-y", "-i", video_path.to_str().unwrap(), "-ss",
               &thumb_time_sec.to_string(), "-frames:v", "1", "-vf",
               r#"scale=if(gte(iw\,ih)\,min(512\,iw)\,-2):if(lt(iw\,ih)\,min(512\,ih)\,-2)"#,
               "-c:v", "libwebp", "-q:v", &config.thumbnail_quality.to_string(),
               thumbnail_path.to_str().unwrap()])
        .stderr(std::process::Stdio::null())
        .status().map_err(
            |e| rterr!("Failed to run ffmpeg to generate thumbnail: {}", e))?;
    if status.success()
    {
        Ok(thumbnail_path)
    }
    else
    {
        Err(rterr!("Ffmpeg failed to generate thumbnail."))
    }
}

fn validateSession(token: &Option<String>, data_manager: &data::Manager,
                   config: &Configuration) -> Result<bool, Error>
{
    if let Some(token) = token
    {
        data_manager.expireSessions(config.session_life_time_sec)?;
        data_manager.hasSession(&token)?;
        Ok(true)
    }
    else
    {
        Ok(false)
    }
}

fn handleIndex(data_manager: &data::Manager, templates: &Tera) ->
    Result<Response, Error>
{
    let videos = data_manager.getVideos(
        "", 0, 1000, data::VideoOrder::NewFirst)?;
    let mut context = tera::Context::new();
    context.insert("videos", &videos);
    Ok(warp::reply::html(templates.render("index.html", &context).map_err(
        |e| rterr!("Failed to render template index.html: {}", e))?)
       .into_response())
}

fn handleVideo(id: String, data_manager: &data::Manager, templates: &Tera) ->
    Result<String, Error>
{
    let video = data_manager.findVideoByID(&id).map_err(|_| Error::HTTPStatus(
        StatusCode::NOT_FOUND, format!("Video {} not found", id)))?;
    let mut context = tera::Context::new();
    context.insert("video", &video);
    templates.render("video.html", &context).map_err(
        |e| rterr!("Failed to render template video.html: {}", e))
}

fn handleUploadPage(data_manager: &data::Manager, templates: &Tera,
                    config: &Configuration, token: Option<String>) ->
    Result<String, Error>
{
    if validateSession(&token, data_manager, config)?
    {
        templates.render("upload.html", &tera::Context::new())
            .map_err(|e| rterr!("Failed to render template upload.html: {}",
                                e))
    }
    else
    {
        Err(Error::HTTPStatus(StatusCode::UNAUTHORIZED, String::new()))
    }
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

async fn videoFromPart(part: warp::multipart::Part, config: &Configuration)
    -> Result<Result<Video, Error>, warp::Error>
{
    if part.filename().is_none()
    {
        return Ok(Err(Error::HTTPStatus(
            StatusCode::BAD_REQUEST, String::from("No filename in upload"))));
    }
    let filename = PathBuf::from(part.filename().unwrap());
    let temp_file = randomTempFilename(&config.video_dir);
    let mut f = match File::create(&temp_file)
    {
        Ok(f) => BufWriter::new(f),
        Err(e) => {
            return Ok(Err(rterr!("Failed to open temp file: {}", e)));
        },
    };
    let mut hasher = sha2::Sha256::new();
    let orig_name: Option<String> = part.filename().map(|n| n.to_owned());
    let mut buffers = part.stream();
    while let Some(buffer) = buffers.next().await
    {
        if buffer.is_err()
        {
            if std::fs::remove_file(&temp_file).is_err()
            {
                log_error!("Failed to remove temp file at {:?}.", temp_file);
            }
        }
        let mut buffer = buffer?;
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
                return Ok(Err(rterr!("Failed to write temp file: {}", e)));
            }
            buffer.advance(bytes.len());
        }
    }
    drop(f);

    let ext = filename.extension().or(Some(OsStr::new(""))).unwrap();
    let hash = hasher.finalize();
    let byte_strs: Vec<_> = hash[..6].iter().map(|b| format!("{:02x}", b))
        .collect();
    let id = byte_strs.join("");
    let video_file: PathBuf = Path::new(&config.video_dir).join(id)
        .with_extension(ext);
    debug!("Moving video {:?} --> {:?}...", temp_file, video_file);
    if let Err(e) = std::fs::rename(&temp_file, &video_file)
    {
        std::fs::remove_file(&temp_file).ok();
        std::fs::remove_file(&video_file).ok();
        return Ok(Err(rterr!("Failed to rename temp file: {}", e)));
    }
    let mut video = Video::fromFile(video_file.file_name().unwrap(),
                                    &config.video_dir).map(
        |v| {
            let mut video = v;
            video.original_filename = orig_name.or(Some(String::new())).unwrap();
            video.upload_time = time::OffsetDateTime::now_utc();
            video
        });
    if video.is_err()
    {
        std::fs::remove_file(&video_file).ok();
    }
    if let Err(e) = generateThumbnail(video.as_ref().unwrap(), config)
    {
        // Thumbnail generation is allowed to fail.
        log_error!("{}", e);
        std::fs::remove_file(&expectedThumbnailPath(video.as_ref().unwrap(),
                                                    config)).ok();
    }
    else
    {
        video = video.map(|mut v| {
            v.thumbnail_path = Some(v.path.with_extension("webp"));
            v
        });
    }
    Ok(video)
}

/// TODO: pipeline the whole upload process.
async fn handleUpload(token: Option<String>,
                      form_data: warp::multipart::FormData,
                      data_manager: &data::Manager,
                      config: &Configuration) ->
    Result<String, warp::Rejection>
{
    if !validateSession(&token, data_manager, config).map_err(
        |_| warp::reject::reject())?
    {
        return Err(warp::reject::reject());
    }
    let parts: Vec<_> = form_data.and_then(
        |part| async move { videoFromPart(part, config).await })
        .try_collect().await.map_err(|e| {
            log_error!("Failed to collect video: {}", e);
            warp::reject::reject()
        })?;
    for part in parts
    {
        let v = part.map_err(|e| {
            log_error!("Failed to deal with part: {}", e);
            warp::reject::reject()
        })?;
        data_manager.addVideo(&v).map_err(|e| {
            log_error!("Failed to add video: {}", e);
            std::fs::remove_file(Path::new(&config.video_dir).join(v.path)).ok();
            warp::reject::reject()
        })?;
        break;
    }
    Ok::<_, warp::Rejection>(String::from("OK"))
}

fn createToken() -> String
{
    BASE64_NO_PAD.encode(rand::random::<i128>().to_ne_bytes())
}

fn uriFromStr(s: &str) -> Result<warp::http::uri::Uri, Error>
{
    s.parse::<warp::http::uri::Uri>().map_err(|_| rterr!("Invalid URI: {}", s))
}

fn makeCookie(token: String, session_life_time: u64) -> String
{
    format!("{}={}; Max-Age={}; Path=/", TOKEN_COOKIE, token, session_life_time)
}

fn handleLogin(auth_value_maybe: Option<String>, data_manager: &data::Manager,
               config: &Configuration) -> Result<Response, Error>
{
    if let Some(auth_value) = auth_value_maybe
    {
        if !auth_value.starts_with("Basic ")
        {
            return Err(Error::HTTPStatus(
                StatusCode::UNAUTHORIZED,
                "Not using basic authentication".to_owned()));
        }
        let expeced = BASE64.encode(format!("user:{}", config.password));
        if expeced.as_str() == &auth_value[6..]
        {
            // Authentication is good.
            let token = createToken();
            data_manager.createSession(&token)?;
            return Ok(warp::reply::with_header(
                warp::redirect::found(uriFromStr(&config.serve_under_path)?),
                "Set-Cookie", makeCookie(token, config.session_life_time_sec))
                      .into_response());
        }
        else
        {
            return Err(Error::HTTPStatus(
                StatusCode::UNAUTHORIZED,
                "Invalid credential".to_owned()));
        }
    }

    Ok(warp::reply::with_header(
        warp::reply::with_status(warp::reply::reply(), StatusCode::UNAUTHORIZED),
        "WWW-Authenticate",
        r#"Basic realm="metube", charset="UTF-8""#).into_response())
}

fn urlEncode(s: &str) -> String
{
    urlencoding::encode(s).to_string()
}

fn urlFor(name: &str, arg: &str) -> String
{
    match name
    {
        "index" => String::from("/"),
        "video" => String::from("/v/") + arg,
        "upload" => String::from("/upload/"),
        "login" => String::from("/login/"),
        "static" => String::from("/static/") + arg,
        "video_file" => String::from("/video/") + arg,
        _ => String::from("/"),
    }
}

fn getTeraFuncArgs(args: &HashMap<String, tera::Value>, arg_name: &str) ->
    tera::Result<String>
{
    let value = args.get(arg_name);
    if value.is_none()
    {
        return Err(format!("Argument {} not found in function call.", arg_name)
                   .into());
    }
    let value: String = tera::from_value(value.unwrap().clone())?;
    Ok(value)
}

fn makeURLFor(serve_path: String) -> impl tera::Function
{
    move |args: &HashMap<String, tera::Value>| ->
        tera::Result<tera::Value> {
            let path_prefix: String = if serve_path == "" || serve_path == "/"
            {
                String::new()
            }
            else if serve_path.starts_with("/")
            {
                serve_path.to_owned()
            }
            else
            {
                String::from("/") + &serve_path
            };

            let name = getTeraFuncArgs(args, "name")?;
            let arg = getTeraFuncArgs(args, "arg")?;
            Ok(tera::to_value(path_prefix + &urlFor(&name, &arg)).unwrap())
    }
}

pub struct App
{
    data_manager: data::Manager,
    templates: Tera,
    config: Configuration,
}

impl App
{
    pub fn new(config: Configuration) -> Result<Self, Error>
    {
        let db_path = Path::new(&config.data_dir).with_file_name("db.sqlite");
        let mut result = Self {
            data_manager: data::Manager::newWithFilename(&db_path),
            templates: Tera::default(),
            config,
        };
        result.init()?;
        Ok(result)
    }

    fn init(&mut self) -> Result<(), Error>
    {
        self.data_manager.connect()?;
        self.data_manager.init()?;
        let template_path = PathBuf::from(&self.config.data_dir)
            .join("templates").canonicalize()
            .map_err(|_| rterr!("Invalid template dir"))?
            .join("**").join("*");
        info!("Template dir is {}", template_path.display());
        let template_dir = template_path.to_str().ok_or_else(
                || rterr!("Invalid template path"))?;
        self.templates = Tera::new(template_dir).map_err(
            |e| rterr!("Failed to compile templates: {}", e))?;
        self.templates.register_function(
            "url_for", makeURLFor(self.config.serve_under_path.clone()));
        Ok(())
    }

    pub async fn serve(self) -> Result<(), Error>
    {
        let static_dir = PathBuf::from(&self.config.static_dir);
        info!("Static dir is {}", static_dir.display());
        let statics = warp::get().and(warp::path("static"))
            .and(warp::fs::dir(static_dir));
        let statics = statics.or(warp::get().and(warp::path("video")).and(
            warp::fs::dir(PathBuf::from(&self.config.video_dir))));

        let data_manager = self.data_manager.clone();
        let temp = self.templates.clone();
        let index = warp::get().and(warp::path::end()).map(move || {
            handleIndex(&data_manager, &temp).toResponse()
        });

        let data_manager = self.data_manager.clone();
        let temp = self.templates.clone();
        let video = warp::get().and(warp::path("v")).and(warp::path::param())
            .and(warp::path::end()).map(move |id: String| {
            handleVideo(id, &data_manager, &temp).toResponse()
        });

        let temp = self.templates.clone();
        let data_manager = self.data_manager.clone();
        let config = self.config.clone();
        let upload_page = warp::get().and(warp::path("upload"))
            .and(warp::path::end())
            .and(warp::filters::cookie::optional(TOKEN_COOKIE)).map(
                move |token: Option<String>|
                handleUploadPage(&data_manager, &temp, &config, token)
                    .toResponse());

        let config = self.config.clone();
        let data_manager = self.data_manager.clone();
        let upload = warp::post().and(warp::path("upload"))
            .and(warp::path::end())
            .and(warp::filters::cookie::optional(TOKEN_COOKIE))
            .and(warp::multipart::form().max_length(self.config.upload_size_max))
            .and_then(move |token: Option<String>, data: warp::multipart::FormData| {
                let config = config.clone();
                let data_manager = data_manager.clone();
                async move {
                    handleUpload(token, data, &data_manager, &config).await
                }
            });

        let config = self.config.clone();
        let data_manager = self.data_manager.clone();
        let login = warp::get().and(warp::path("login")).and(warp::path::end())
            .and(warp::header::optional::<String>("Authorization"))
            .map(move |auth_value: Option<String>| {
                handleLogin(auth_value, &data_manager, &config).toResponse()
            });

        let route = if self.config.serve_under_path == String::from("/") ||
            self.config.serve_under_path.is_empty()
        {
            statics.or(index).or(video).or(upload_page).or(upload).or(login)
                .boxed()
        }
        else
        {
            let mut segs = self.config.serve_under_path.split('/');
            if self.config.serve_under_path.starts_with("/")
            {
                segs.next();
            }
            let first: String = segs.next().unwrap().to_owned();
            let mut r = warp::path(first).boxed();
            for seg in segs
            {
                r = r.and(warp::path(seg.to_owned())).boxed();
            }
            r.and(statics.or(index).or(video).or(upload_page).or(upload)
                  .or(login))
                .boxed()
        };

        info!("Listening at {}:{}...", self.config.listen_address,
              self.config.listen_port);

        warp::serve(route).run(
            std::net::SocketAddr::new(
                self.config.listen_address.parse().map_err(
                    |_| rterr!("Invalid listen address: {}",
                               self.config.listen_address))?,
                self.config.listen_port)).await;
        Ok(())
    }
}
