//! The env-proxy convention on the ONE-SHOT path (search engines, verticals,
//! every single-hop rider). Its own test binary on purpose: the assertion
//! needs process env vars, and a parallel lib test that dials a local rig
//! would otherwise inherit the proxy mid-flight.

use donsetch::fetch::client::Fetcher;
use donsetch::profile::BrowserProfile;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

/// A minimal forward proxy: it answers the absolute-form GET that a
/// plain-HTTP proxy hop carries, and reports the request line back so the test
/// can prove which lane the fetch actually took.
fn proxy_rig() -> (u16, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok((mut sock, _)) = listener.accept() {
            let _ = sock.set_read_timeout(Some(Duration::from_secs(6)));
            let Ok(clone) = sock.try_clone() else { break };
            let mut reader = BufReader::new(clone);
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if line == "\r\n" {
                    break;
                }
            }
            let body = "<html><head><title>via-proxy</title></head><body>a search engine response that rode the environment proxy lane.</body></html>";
            let _ = sock.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = tx.send(request_line);
        }
    });
    (port, rx)
}

/// A host whose only egress is `HTTP_PROXY` must not dial the origin on the
/// one-shot path, and `NO_PROXY` must still exempt it.
#[tokio::test]
async fn one_shot_fetch_rides_the_environment_proxy() {
    let (port, requests) = proxy_rig();
    unsafe {
        std::env::set_var("DONSETCH_ALLOW_PRIVATE_EGRESS", "1");
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("no_proxy");
        std::env::set_var("HTTP_PROXY", format!("http://127.0.0.1:{port}"));
    }
    let fetcher = Fetcher::new(BrowserProfile::host_default()).unwrap();
    let outcome = fetcher
        .fetch_once_via("http://serp.invalid/search?q=dexa", &[], None, false, None)
        .await;
    assert!(outcome.is_ok(), "the proxy lane answered");
    let line = requests
        .recv_timeout(Duration::from_secs(5))
        .expect("the one-shot fetch reached the environment proxy");
    assert!(
        line.starts_with("GET http://serp.invalid/search?q=dexa "),
        "a proxy hop carries the absolute form: {line:?}"
    );

    // NO_PROXY still wins: the fetch dials the origin, which does not resolve,
    // and the proxy sees nothing more.
    unsafe { std::env::set_var("NO_PROXY", "serp.invalid") };
    let direct = fetcher
        .fetch_once_via("http://serp.invalid/search?q=dexa", &[], None, false, None)
        .await;
    assert!(direct.is_err(), "NO_PROXY exempts the host");
    assert!(
        requests.recv_timeout(Duration::from_millis(300)).is_err(),
        "an exempt host never reaches the proxy"
    );
    unsafe {
        std::env::remove_var("HTTP_PROXY");
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("DONSETCH_ALLOW_PRIVATE_EGRESS");
    }
}
