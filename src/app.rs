use std::path::{PathBuf, Path};
use std::collections::HashMap;

use futures_util::TryStreamExt;
use log::info;
use log::error as log_error;
use tera::Tera;
use warp::{Filter, Reply};
use warp::http::status::StatusCode;
use warp::reply::Response;
use base64::engine::Engine;

use crate::error;
use crate::error::Error;
use crate::config::Configuration;
use crate::data;
use crate::video_processing::UploadingVideo;

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

fn handleIndex(data_manager: &data::Manager, templates: &Tera,
               config: &Configuration) -> Result<Response, Error>
{
    let videos = data_manager.getVideos(
        0, 1000, data::VideoOrder::NewFirst)?;
    let mut context = tera::Context::new();
    context.insert("videos", &videos);
    context.insert("site_info", &config.site_info);
    Ok(warp::reply::html(templates.render("index.html", &context).map_err(
        |e| rterr!("Failed to render template index.html: {}", e))?)
       .into_response())
}

fn handleVideo(id: String, data_manager: &data::Manager, templates: &Tera,
               config: &Configuration) -> Result<String, Error>
{
    let video = data_manager.findVideoByID(&id).map_err(|_| Error::HTTPStatus(
        StatusCode::NOT_FOUND, format!("Video {} not found", id)))?;
    let mut context = tera::Context::new();
    context.insert("video", &video);
    context.insert("site_info", &config.site_info);
    let res = templates.render("video.html", &context).map_err(
        |e| rterr!("Failed to render template video.html: {}", e));
    if let Err(e) = data_manager.increaseViewCount(&id)
    {
        log_error!("{}", e);
    }
    res
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
    // let parts: Vec<_> = form_data.and_then(
    //     |part| async move { videoFromPart(part, config).await })
    //     .try_collect().await.map_err(|e| {
    //         log_error!("Failed to collect video: {}", e);
    //         warp::reject::reject()
    //     })?;
    let parts: Vec<_> = form_data.and_then(
        |part| async move {
            let v = UploadingVideo { part };
            let v = v.saveToTemp(config).await;
            // v is a Result<_, error::Error>. But this async stream
            // thing requires a Result<_, warp::Error>. So here we
            // just wrap a extra layer of Result<_, warp::Error>.
            // Later we will just unwrap it.
            Ok(v)
        }).try_collect().await
        // Unwrap the Result<_, warp::Error> here.
        .unwrap();

    for part in parts
    {
        part.map_err(error::reject)?
            .moveToLibrary(config).map_err(error::reject)?
            .makeRelativePath(config).map_err(error::reject)?
            .probeMetadata(config).map_err(error::reject)?
            .generateThumbnail(config).map_err(error::reject)?
            .addToDatabase(config, data_manager).map_err(error::reject)?;
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
        let db_path = Path::new(&config.data_dir).join("db.sqlite");
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
        let config = self.config.clone();
        let index = warp::get().and(warp::path::end()).map(move || {
            handleIndex(&data_manager, &temp, &config).toResponse()
        });

        let data_manager = self.data_manager.clone();
        let temp = self.templates.clone();
        let config = self.config.clone();
        let video = warp::get().and(warp::path("v")).and(warp::path::param())
            .and(warp::path::end()).map(move |id: String| {
            handleVideo(id, &data_manager, &temp, &config).toResponse()
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
