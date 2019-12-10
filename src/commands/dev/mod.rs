mod server_config;
mod socket;
use server_config::ServerConfig;

extern crate openssl;
extern crate tokio;
extern crate url;
extern crate ws;

use std::thread;

use chrono::prelude::*;

use hyper::client::{HttpConnector, ResponseFuture};
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client as HyperClient, Request, Response, Server, Uri};
use hyper::http::uri::InvalidUri;

use hyper_tls::HttpsConnector;

use tokio::runtime::Runtime;

use uuid::Uuid;

use crate::commands;
use crate::commands::preview::upload;

use crate::settings::global_user::GlobalUser;
use crate::settings::target::Target;

use crate::terminal::emoji;

const PREVIEW_HOST: &str = "rawhttp.cloudflareworkers.com";
const HEADER_PREFIX: &str = "cf-ew-raw-";

pub fn dev(
    target: Target,
    user: Option<GlobalUser>,
    host: Option<&str>,
    port: Option<&str>,
    ip: Option<&str>,
) -> Result<(), failure::Error> {
    commands::build(&target)?;
    let server_config = ServerConfig::new(host, ip, port)?;
    let session_id = get_session_id()?;
    let preview_id = get_preview_id(target, user, &server_config, &session_id)?;

    thread::spawn(move || socket::listen(session_id));

    let mut rt = Runtime::new()?;
    rt.block_on(serve(server_config, preview_id))?;

    Ok(())
}

async fn serve(server_config: ServerConfig, preview_id: String) -> Result<(), failure::Error> {
    // set up https client to connect to the preview service
    let https = HttpsConnector::new();
    let client = HyperClient::builder().build::<_, Body>(https);

    let listening_address = server_config.listening_address.clone();
    // create a closure that hyper will use later to handle HTTP requests
    let make_service = make_service_fn(move |_| {
        let client = client.clone();
        let preview_id = preview_id.to_owned();
        let server_config = server_config.clone();
        async move {
            Ok::<_, failure::Error>(service_fn(move |req| {
                let client = client.to_owned();
                let preview_id = preview_id.to_owned();
                let server_config = server_config.clone();
                async move {
                    let resp = preview_request(req, client, preview_id, server_config).await?;

                    let (mut parts, body) = resp.into_parts();

                    let mut headers = HeaderMap::new();

                    for header in &parts.headers {
                        let (name, value) = header;
                        let name = name.as_str();
                        if name.starts_with(HEADER_PREFIX) {
                            let header_name = &name[HEADER_PREFIX.len()..];
                            // TODO: remove unwrap
                            let header_name =
                                HeaderName::from_bytes(header_name.as_bytes()).unwrap();
                            headers.insert(header_name, value.clone());
                        }
                    }
                    parts.headers = headers;

                    let resp = Response::from_parts(parts, body);
                    Ok::<_, failure::Error>(resp)
                }
            }))
        }
    });

    let server = Server::bind(&listening_address.address).serve(make_service);
    println!("{} Listening on http://{}", emoji::EAR, listening_address);
    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
    Ok(())
}

fn get_preview_url(path_string: &str) -> Result<Uri, InvalidUri> {
    format!("https://{}{}", PREVIEW_HOST, path_string).parse()
}

fn get_path_as_str(uri: Uri) -> String {
    uri.path_and_query()
        .map(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn preview_request(
    req: Request<Body>,
    client: HyperClient<HttpsConnector<HttpConnector>>,
    preview_id: String,
    server_config: ServerConfig,
) -> ResponseFuture {
    let (mut parts, body) = req.into_parts();

    let path = get_path_as_str(parts.uri);
    let method = parts.method.to_string();
    let now: DateTime<Local> = Local::now();
    let preview_id = &preview_id;

    let mut headers: HeaderMap = HeaderMap::new();

    for header in &parts.headers {
        let (name, value) = header;
        let forward_header = format!("{}{}", HEADER_PREFIX, name);
        // TODO: remove unwrap
        let header_name = HeaderName::from_bytes(forward_header.as_bytes()).unwrap();
        headers.insert(header_name, value.clone());
    }
    parts.headers = headers;

    // TODO: remove unwrap
    parts.uri = get_preview_url(&path).unwrap();
    parts.headers.insert(
        HeaderName::from_static("host"),
        HeaderValue::from_static(PREVIEW_HOST),
    );

    // TODO: remove unwrap
    parts.headers.insert(
        HeaderName::from_static("cf-ew-preview"),
        HeaderValue::from_str(preview_id).unwrap(),
    );

    let req = Request::from_parts(parts, body);

    println!(
        "[{}] \"{} {}{} {:?}\"",
        now.format("%Y-%m-%d %H:%M:%S"),
        method,
        server_config.host,
        path,
        req.version()
    );
    client.request(req)
}

fn get_session_id() -> Result<String, failure::Error> {
    Ok(Uuid::new_v4().to_simple().to_string())
}

fn get_preview_id(
    mut target: Target,
    user: Option<GlobalUser>,
    server_config: &ServerConfig,
    session_id: &str,
) -> Result<String, failure::Error> {
    let verbose = true;
    let sites_preview = false;
    let script_id: String = upload(&mut target, user.as_ref(), sites_preview, verbose)?;
    Ok(format!(
        "{}{}{}{}",
        &script_id,
        session_id,
        server_config.host.is_https() as u8,
        server_config.host
    ))
}
