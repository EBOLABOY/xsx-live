use anyhow::{Context, Result};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{body::{Bytes, Incoming}, header, server::conn::http1, service::service_fn, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use std::{collections::HashMap, convert::Infallible, path::PathBuf, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::{watch, Mutex}};
use crate::control::{Command, ControlState};

struct App {
    host: String,
    origin: String,
    csrf: String,
    control: watch::Sender<ControlState>,
    mutation: Mutex<()>,
    state_dir: PathBuf,
    files: HashMap<&'static str, (&'static str, Bytes)>,
    http: reqwest::Client,
    media_base: String,
    rtmp: String,
}
type Reply = Response<Full<Bytes>>;
fn response(status: StatusCode, mime: &str, body: impl Into<Bytes>) -> Reply {
    Response::builder().status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::CONNECTION, "close")
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "same-origin")
        .header("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; frame-src 'self' http: https:; object-src 'none'; base-uri 'none'; frame-ancestors 'self'")
        .body(Full::new(body.into())).expect("valid response")
}
fn json_response(status: StatusCode, body: Value) -> Reply {
    response(status, "application/json; charset=utf-8", serde_json::to_vec(&body).expect("JSON response"))
}
fn error(status: StatusCode, message: &str) -> Reply {
    json_response(status, json!({"error":message}))
}

pub async fn start(control: watch::Sender<ControlState>) -> Result<()> {
    let bind = std::env::var("DASHBOARD_BIND").unwrap_or_else(|_| "0.0.0.0:8099".into());
    let origin = format!("http://{bind}");
    let dir = PathBuf::from(std::env::var("DASHBOARD_DIR").unwrap_or_else(|_| "/dashboard".into()));
    let mut files = HashMap::new();
    for (route, name, mime) in [
        ("/", "index.html", "text/html; charset=utf-8"),
        ("/app.js", "app.js", "text/javascript; charset=utf-8"),
        ("/style.css", "style.css", "text/css; charset=utf-8"),
    ] {
        files.insert(route, (mime, Bytes::from(std::fs::read(dir.join(name)).context("Dashboard assets missing")?)));
    }
    let mut random = [0u8; 32]; getrandom::fill(&mut random).expect("OS randomness");
    let csrf = random.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let listener = TcpListener::bind(&bind).await.context("Dashboard listen")?;
    let app = Arc::new(App {
        host: bind, origin, csrf, control, mutation: Mutex::new(()),
        state_dir: PathBuf::from(std::env::var("STATE_DIR").unwrap_or_else(|_| "/state".into())),
        files, http: reqwest::Client::builder().timeout(Duration::from_secs(2)).build()?,
        media_base: std::env::var("MEDIA_HTTP_BASE").unwrap_or_else(|_| "http://127.0.0.1:8888".into()),
        rtmp: std::env::var("OUTPUT_RTMP").unwrap_or_else(|_| "rtmp://127.0.0.1:1935/live/xsx".into()),
    });
    println!("DASHBOARD listening={}", app.origin);
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => { tokio::time::sleep(Duration::from_secs(1)).await; continue; }
            };
            let state = app.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let service = service_fn(move |req| handle(req, state.clone()));
                let mut builder = http1::Builder::new();
                builder.keep_alive(false).max_buf_size(16384);
                let _ = tokio::time::timeout(Duration::from_secs(10), builder.serve_connection(io, service)).await;
            });
        }
    });
    Ok(())
}

fn origin_allowed(headers: &hyper::HeaderMap, app: &App) -> bool {
    headers.get(header::HOST).and_then(|v| v.to_str().ok()) == Some(app.host.as_str())
        && headers.get(header::ORIGIN).is_none_or(|v| v.to_str().ok() == Some(app.origin.as_str()))
}

async fn status(app: &App) -> Value {
    let receiver: Value = std::fs::read(app.state_dir.join("status.json")).ok()
        .and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(json!({"state":"unknown"}));
    let control = app.control.borrow().clone();
    let media = match app.http.get("http://127.0.0.1:1985/api/v1/streams/").send().await {
        Ok(r) => match r.json::<Value>().await {
            Ok(v) => v["streams"].as_array().and_then(|items| items.iter().find(|s|
                s["app"] == "live" && s["name"] == "xsx" && s["publish"]["active"] == true
            )).map(|s| json!({"active":true,"video":s["video"],"audio":s["audio"],"kbps":s["kbps"]["recv_30s"],"clients":s["clients"]})).unwrap_or(json!({"active":false})),
            Err(_) => json!({"active":false}),
        },
        Err(_) => json!({"active":false}),
    };
    let base = app.media_base.trim_end_matches('/');
    json!({
        "now":chrono::Utc::now().timestamp(),
        "control":control,
        "receiver":receiver,
        "media":media,
        "csrf_token":app.csrf,
        "links":{
            "dashboard":format!("{}/",app.origin),
            "browser":format!("{base}/xsx/index.html"),
            "flv":format!("{base}/live/xsx.flv"),
            "rtmp":app.rtmp,
            "preview":format!("{base}/xsx/index.html?audio=0")
        }
    })
}

async fn handle(req: Request<Incoming>, app: Arc<App>) -> Result<Reply, Infallible> {
    if !origin_allowed(req.headers(), &app) {
        return Ok(error(StatusCode::FORBIDDEN, "不允许的访问来源"));
    }
    let path = req.uri().path().to_owned();
    if req.method() == Method::GET {
        if path == "/api/status" { return Ok(json_response(StatusCode::OK, status(&app).await)); }
        if path == "/favicon.ico" { return Ok(response(StatusCode::NO_CONTENT, "image/x-icon", Bytes::new())); }
        if let Some((mime, data)) = app.files.get(path.as_str()) {
            return Ok(response(StatusCode::OK, mime, data.clone()));
        }
        return Ok(error(StatusCode::NOT_FOUND, "页面不存在"));
    }
    if req.method() != Method::POST || path != "/api/control" {
        return Ok(error(StatusCode::METHOD_NOT_ALLOWED, "不支持的操作"));
    }
    if req.headers().get("X-XSX-Token").and_then(|v|v.to_str().ok()) != Some(app.csrf.as_str()) {
        return Ok(error(StatusCode::FORBIDDEN, "页面已更新，请刷新后再操作"));
    }
    if !req.headers().get(header::CONTENT_TYPE).and_then(|v|v.to_str().ok()).is_some_and(|v|v.split(';').next()==Some("application/json")) {
        return Ok(error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "需要 JSON 请求"));
    }
    let body = match Limited::new(req.into_body(), 1024).collect().await {
        Ok(v) => v.to_bytes(),
        Err(_) => return Ok(error(StatusCode::PAYLOAD_TOO_LARGE, "请求过大")),
    };
    let command: Command = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Ok(error(StatusCode::BAD_REQUEST, "不支持的控制指令")),
    };
    let _guard = app.mutation.lock().await;
    let before = app.control.borrow().clone();
    let after = before.apply(command.action);
    let changed = after != before;
    if changed {
        if after.save().is_err() { return Ok(error(StatusCode::INTERNAL_SERVER_ERROR, "保存开关状态失败")); }
        app.control.send_replace(after.clone());
        println!("CONTROL enabled={} revision={}",after.enabled,after.revision);
    }
    Ok(json_response(if changed {StatusCode::ACCEPTED} else {StatusCode::OK},
        json!({"accepted":true,"changed":changed,"control":after})))
}
