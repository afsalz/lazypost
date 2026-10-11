//! Executes a request (blocking; called from a worker thread).

use crate::model::{substitute, AuthKind, Request};
use base64::Engine;
use std::{
    collections::HashMap,
    io::Read,
    time::{Duration, Instant},
};

const MAX_BODY: u64 = 20 * 1024 * 1024;

pub struct Response {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub json: bool,
    pub elapsed: Duration,
    pub size: usize,
}

pub fn send(req: &Request, vars: &HashMap<String, String>) -> Result<Response, String> {
    let sub = |s: &str| substitute(s, vars);
    let mut url = sub(req.url.trim());
    if let Some(i) = url.find("{{") {
        let var = url[i..].split_inclusive("}}").next().unwrap_or(&url[i..]);
        return Err(format!("Undefined variable {var} — define it in the collection's .env file"));
    }
    if !url.contains("://") {
        url = format!("http://{url}");
    }
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(60)).build();
    let mut r = agent.request(&req.method, &url);
    for p in req.params.iter().filter(|p| p.enabled && !p.key.is_empty()) {
        r = r.query(&sub(&p.key), &sub(&p.value));
    }
    let mut has_content_type = false;
    for h in req.headers.iter().filter(|h| h.enabled && !h.key.is_empty()) {
        has_content_type |= h.key.eq_ignore_ascii_case("content-type");
        r = r.set(sub(&h.key).trim(), sub(&h.value).trim());
    }
    match req.auth.kind {
        AuthKind::None => {}
        AuthKind::Basic => {
            let raw = format!("{}:{}", sub(&req.auth.username), sub(&req.auth.password));
            let enc = base64::engine::general_purpose::STANDARD.encode(raw);
            r = r.set("Authorization", &format!("Basic {enc}"));
        }
        AuthKind::Bearer => r = r.set("Authorization", &format!("Bearer {}", sub(&req.auth.token))),
    }
    let body = sub(&req.body);
    let start = Instant::now();
    let result = if body.trim().is_empty() {
        r.call()
    } else {
        if !has_content_type {
            let json = serde_json::from_str::<serde_json::Value>(&body).is_ok();
            r = r.set("Content-Type", if json { "application/json" } else { "text/plain" });
        }
        r.send_string(&body)
    };
    let resp = match result {
        Ok(resp) | Err(ureq::Error::Status(_, resp)) => resp,
        Err(e) => return Err(e.to_string()),
    };
    let status = resp.status();
    let status_text = resp.status_text().to_string();
    let mut names = resp.headers_names();
    names.dedup();
    let mut headers = Vec::new();
    for name in names {
        for v in resp.all(&name) {
            headers.push((name.clone(), v.to_string()));
        }
    }
    let mut bytes = Vec::new();
    resp.into_reader().take(MAX_BODY).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let elapsed = start.elapsed();
    let size = bytes.len();
    let mut body = String::from_utf8_lossy(&bytes).into_owned();
    let mut json = false;
    if matches!(body.trim_start().chars().next(), Some('{') | Some('[')) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            body = serde_json::to_string_pretty(&v).unwrap_or(body);
            json = true;
        }
    }
    Ok(Response { status, status_text, headers, body, json, elapsed, size })
}
