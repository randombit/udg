//! `udg serve`: a minimal, dependency-free static file server for the
//! built docs. No file watching — rerun `udg build` and refresh.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;

use anyhow::{Context, Result};

pub fn serve(dir: &Path, port: u16) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("cannot bind 127.0.0.1:{port}"))?;
    eprintln!("udg: serving {} at http://127.0.0.1:{port}/", dir.display());
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            continue;
        }
        // Drain headers.
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok() && line.trim() != "" {
            line.clear();
        }
        let path = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or("/")
            .split(['?', '#'])
            .next()
            .unwrap_or("/");
        let rel = path.trim_start_matches('/');
        let rel = if rel.is_empty() { "index.html" } else { rel };

        // Refuse traversal; serve only files inside dir.
        let target = dir.join(rel);
        let ok = target
            .canonicalize()
            .ok()
            .filter(|t| t.starts_with(dir))
            .filter(|t| t.is_file());
        let response = match ok.and_then(|t| std::fs::read(&t).ok().map(|b| (t, b))) {
            Some((t, body)) => {
                let mime = match t.extension().and_then(|e| e.to_str()) {
                    Some("html") => "text/html; charset=utf-8",
                    Some("css") => "text/css",
                    Some("js") => "application/javascript",
                    Some("json") => "application/json",
                    Some("svg") => "image/svg+xml",
                    _ => "application/octet-stream",
                };
                let mut r = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                r.extend(body);
                r
            }
            None => {
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found"
                    .to_vec()
            }
        };
        let _ = stream.write_all(&response);
    }
    Ok(())
}
