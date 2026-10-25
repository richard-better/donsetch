//! `donsetch fetch --json` end to end: the page's main image shows up as an
//! `image:` line right under the source URL and as `meta.image`, on the
//! block-extraction path and on the raw-text fallback path, and pages
//! without an image keep their output.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

const SHOP_HEAD: &str = r#"<title>Kenya Lot 20</title>
<meta property="og:image" content="http://cdn.example.com/plain.png">
<meta property="og:image:secure_url" content="https://cdn.example.com/secure.png?v=1">"#;

fn paragraphs() -> String {
    (0..12)
        .map(|i| {
            format!(
                "<p>Paragraph {i}: a washed Kenyan lot with notes of blackcurrant, orange and brown sugar, \
                 roasted for filter and sold in small batches by a roaster that cares.</p>"
            )
        })
        .collect()
}

/// A page whose text sits in bare divs, so block extraction gives up and
/// the raw-text fallback produces the markdown.
fn div_soup() -> String {
    (0..12)
        .map(|i| {
            format!(
                "<div>Row {i} of loose text without any paragraph markup, long enough to count as real page copy for the fallback.</div>"
            )
        })
        .collect()
}

fn serve() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            let body = match path.as_str() {
                "/article" => format!(
                    "<html><head>{SHOP_HEAD}</head><body><main><h1>Kenya Lot 20</h1>{}</main></body></html>",
                    paragraphs()
                ),
                "/soup" => format!(
                    "<html><head>{SHOP_HEAD}</head><body>{}</body></html>",
                    div_soup()
                ),
                _ => format!(
                    "<html><head><title>No Image</title></head><body><main><h1>No Image</h1>{}</main></body></html>",
                    paragraphs()
                ),
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

fn fetch_json(port: u16, path: &str) -> serde_json::Value {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_donsetch"));
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("DONSETCH_") {
            cmd.env_remove(&k);
        }
    }
    let out = cmd
        .env("DONSETCH_NO_CONFIG_FILE", "1")
        .env("DONSETCH_ALLOW_PRIVATE_EGRESS", "1")
        .args([
            "fetch",
            "--tier",
            "1",
            "--max-chars",
            "1048576",
            "--offset",
            "0",
            "--archive",
            "off",
            "--deadline-ms",
            "15000",
            "--json",
            "--",
            &format!("http://127.0.0.1:{port}{path}"),
        ])
        .output()
        .expect("run donsetch");
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

const IMAGE: &str = "https://cdn.example.com/secure.png?v=1";

fn assert_image_under_url(v: &serde_json::Value, port: u16, path: &str) {
    assert_eq!(v["ok"], true, "{v}");
    let content = v["content"].as_str().unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(
        lines[1],
        format!("http://127.0.0.1:{port}{path}"),
        "{content}"
    );
    assert_eq!(lines[2], format!("image: {IMAGE}"), "{content}");
    assert_eq!(content.matches("image: ").count(), 1, "{content}");
    assert_eq!(v["meta"]["image"], IMAGE, "{v}");
}

#[test]
fn fetch_reports_the_image_for_block_extracted_pages() {
    let port = serve();
    let v = fetch_json(port, "/article");
    assert_image_under_url(&v, port, "/article");
}

#[test]
fn fetch_reports_the_image_for_raw_text_fallback_pages() {
    let port = serve();
    let v = fetch_json(port, "/soup");
    assert_image_under_url(&v, port, "/soup");
}

#[test]
fn fetch_without_an_image_prints_none() {
    let port = serve();
    let v = fetch_json(port, "/plain");
    assert_eq!(v["ok"], true, "{v}");
    assert!(!v["content"].as_str().unwrap().contains("image: "));
    assert!(v["meta"].get("image").is_none());
}
