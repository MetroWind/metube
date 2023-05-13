use std::path::{PathBuf, Path};
use std::collections::HashMap;
use std::io::prelude::*;
use std::io::BufWriter;
use std::fs::File;
use std::ffi::OsStr;

use futures_util::TryStreamExt;
use futures_util::StreamExt;
use bytes::buf::Buf;
use log::{info, debug};
use log::error as log_error;
use tera::Tera;
use warp::{Filter, Reply};
use warp::http::status::StatusCode;
use warp::reply::Response;
use sha2::Digest;

use crate::error::Error;
use crate::config::Configuration;
use crate::data;
use crate::video::Video;

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
                warp::reply::with_status(
                e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
                    .into_response()
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
            Ok(s) => s.into_response(),
            Err(e) => {
                log_error!("{}", e);
                warp::reply::with_status(
                e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
                    .into_response()
            }
        }
     }
}

fn handleIndex(data_manager: &data::Manager, templates: &Tera) ->
    Result<String, Error>
{
    let videos = data_manager.getVideos(
        "", 0, 1000, data::VideoOrder::NewFirst)?;
    let mut context = tera::Context::new();
    context.insert("videos", &videos);
    templates.render("index.html", &context).map_err(
        |e| rterr!("Failed to render template index.html: {}", e))
}

fn handleUploadPage(templates: &Tera) -> Result<String, Error>
{
    templates.render("upload.html", &tera::Context::new()).map_err(
        |e| rterr!("Failed to render template upload.html: {}", e))
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
        return Ok(Err(Error::HTTPStatus(400)));
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
    let mut buffers = part.stream();
    while let Some(buffer) = buffers.next().await
    {
        let mut buffer = buffer?;
        while buffer.has_remaining()
        {
            let bytes = buffer.chunk();
            hasher.update(bytes);
            if let Err(e) = f.write_all(bytes)
            {
                return Ok(Err(rterr!("Failed to write temp file: {}", e)));
            }
            buffer.advance(bytes.len());
        }
    }
    let ext = filename.extension().or(Some(OsStr::new(""))).unwrap();
    let hash = hasher.finalize();
    let byte_strs: Vec<_> = hash[..6].iter().map(|b| format!("{:02x}", b))
        .collect();
    let id = byte_strs.join("");
    let video_file: PathBuf = Path::new(&config.video_dir).join(id)
        .with_extension(ext);
    if let Err(e) = std::fs::rename(temp_file, &video_file)
    {
        return Ok(Err(rterr!("Failed to rename temp file: {}", e)));
    }
    Ok(Video::fromFile(video_file.file_name().unwrap(), &config.video_dir))
}

async fn handleUpload(data: warp::multipart::FormData,
                      config: &Configuration) ->
    Result<String, warp::Rejection>
{
    println!("Handling upload...");
    let parts: Vec<_> = data.and_then(
        |mut part| async move { videoFromPart(part, config).await })
        .try_collect().await.map_err(|e| warp::reject::reject())?;
    for part in parts
    {
        if let Err(_) = part
        {
            return Err(warp::reject::reject());
        }
    }
    Ok::<_, warp::Rejection>(String::from("OK"))
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
        "static" => String::from("/static/") + arg,
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

        let temp = self.templates.clone();
        let upload_page = warp::get().and(warp::path("upload"))
            .and(warp::path::end()).map(
                move || handleUploadPage(&temp).toResponse());

        let config = self.config.clone();
        let upload = warp::post().and(warp::path("upload"))
            .and(warp::multipart::form().max_length(self.config.upload_size_max))
            .and_then(move |data: warp::multipart::FormData| {
                let config = config.clone();
                async move {
                    handleUpload(data, &config).await
                }
            });

        // let data = self.data.clone();
        // let temp = self.templates.clone();
        // let family = warp::path("family").and(warp::path::param()).map(
        //     move |param: String| {
        //         handleFamily(param, &data, &temp).toResponse()
        //     });

        let route = if self.config.serve_under_path == String::from("/") ||
            self.config.serve_under_path.is_empty()
        {
            statics.or(index).or(upload_page).or(upload).boxed()
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
            r.and(statics.or(index).or(upload_page).or(upload)).boxed()
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
