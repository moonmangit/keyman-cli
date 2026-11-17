//! Browser sign-in: PKCE and a local loopback listener.

use super::account::{Prompt, SIGN_IN_CANCELLED};
use super::api::{self, Session};
use super::config::Config;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

/// The redirect URLs on the server allow-list use these ports.
pub const CALLBACK_PORTS: [u16; 5] = [47821, 47822, 47823, 47824, 47825];

const WAIT: Duration = Duration::from_secs(300);
const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub fn pkce() -> Pkce {
    let n = UNRESERVED.len(); // 66
    let limit = 256 - (256 % n);
    let mut verifier = String::with_capacity(64);
    while verifier.len() < 64 {
        let mut buf = [0u8; 64];
        getrandom::getrandom(&mut buf).expect("system random source failed");
        for b in buf {
            // reject biased values
            if (b as usize) < limit && verifier.len() < 64 {
                verifier.push(UNRESERVED[b as usize % n] as char);
            }
        }
    }
    let challenge = challenge_for(&verifier);
    Pkce {
        verifier,
        challenge,
    }
}

pub fn authorize_url(cfg: &Config, provider: &str, redirect: &str, challenge: &str) -> String {
    format!(
        "{}/auth/v1/authorize?provider={}&redirect_to={}&code_challenge={}&code_challenge_method=s256",
        cfg.url,
        api::urlenc(provider),
        api::urlenc(redirect),
        challenge
    )
}

fn urldec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Request target of a `GET` request line, when it is the callback path.
fn callback_query(request_line: &str) -> Option<&str> {
    let target = request_line.split_whitespace().nth(1)?;
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    (path == "/callback").then_some(query)
}

/// Make server text safe for a terminal: no control characters, limited length.
fn clean_text(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(200).collect()
}

pub fn parse_callback(request_line: &str) -> Result<String, String> {
    let query = callback_query(request_line).ok_or("The sign-in reply is not valid.")?;
    let (mut code, mut error, mut desc) = (None, None, None);
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "code" => code = Some(urldec(v)),
            "error" => error = Some(urldec(v)),
            "error_description" => desc = Some(urldec(v)),
            _ => {}
        }
    }
    if let Some(d) = desc.or(error) {
        return Err(format!("The sign-in failed: {}", clean_text(&d)));
    }
    code.filter(|c| !c.is_empty())
        .ok_or_else(|| "The sign-in reply has no code.".to_string())
}

/// A complete HTTP/1.1 reply with a small HTML page.
fn http_response(status: &str, text: &str) -> String {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Keyman</title></head><body style=\"font-family:sans-serif;margin:3em\"><p>{text}</p></body></html>"
    );
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn reply(stream: &mut std::net::TcpStream, status: &str, text: &str) {
    let _ = stream.write_all(http_response(status, text).as_bytes());
    let _ = stream.flush();
}

/// Read the request line, at most 8 KiB.
fn read_request_line(stream: &std::net::TcpStream) -> Option<String> {
    let mut line = String::new();
    BufReader::new(stream.take(8192))
        .read_line(&mut line)
        .ok()?;
    Some(line)
}

fn is_get(request_line: &str) -> bool {
    request_line.split_whitespace().next() == Some("GET")
}

fn bind_callback_port() -> Result<(TcpListener, u16), String> {
    for port in CALLBACK_PORTS {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", port)) {
            return Ok((l, port));
        }
    }
    Err("Keyman cannot open a local port for sign-in. Close other programs that use ports 47821 to 47825, then try again.".to_string())
}

/// Wait for the browser to call back and return the request line. The
/// prompt can cancel the wait.
fn wait_for_callback(
    listener: &TcpListener,
    prompt: &dyn Prompt,
    deadline: Instant,
) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Keyman cannot wait for sign-in: {e}"))?;
    loop {
        if prompt.cancelled() {
            return Err(SIGN_IN_CANCELLED.to_string());
        }
        if Instant::now() >= deadline {
            return Err("Sign-in took too long. Try again.".to_string());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let Some(line) = read_request_line(&stream) else {
                    continue;
                };
                if !is_get(&line) {
                    reply(&mut stream, "405 Method Not Allowed", "Not allowed.");
                    continue;
                }
                if callback_query(&line).is_none() {
                    reply(&mut stream, "404 Not Found", "Not found.");
                    continue;
                }
                match parse_callback(&line) {
                    Ok(_) => reply(
                        &mut stream,
                        "200 OK",
                        "Sign-in is complete. You can close this window.",
                    ),
                    Err(_) => reply(
                        &mut stream,
                        "200 OK",
                        "Sign-in failed. You can close this window.",
                    ),
                }
                return Ok(line);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("Keyman cannot wait for sign-in: {e}")),
        }
    }
}

/// Wait for the browser and return the sign-in code.
fn wait_for_code(
    listener: &TcpListener,
    prompt: &dyn Prompt,
    deadline: Instant,
) -> Result<String, String> {
    let line = wait_for_callback(listener, prompt, deadline)?;
    parse_callback(&line)
}

pub fn sign_in_with_browser(
    cfg: &Config,
    provider: &str,
    prompt: &mut dyn Prompt,
) -> Result<Session, String> {
    let (listener, port) = bind_callback_port()?;
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let pk = pkce();
    let url = authorize_url(cfg, provider, &redirect, &pk.challenge);
    prompt.sign_in_link(&url);
    let _ = webbrowser::open(&url);
    let code = wait_for_code(&listener, prompt, Instant::now() + WAIT)?;
    api::exchange_code(cfg, &code, &pk.verifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc7636_example() {
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn pkce_verifier_is_valid() {
        let p = pkce();
        assert_eq!(p.verifier.len(), 64);
        assert!(p
            .verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c)));
        assert_eq!(p.challenge, challenge_for(&p.verifier));
    }

    #[test]
    fn authorize_url_encodes_redirect() {
        let cfg = Config {
            url: "https://x.supabase.co".into(),
            key: "k".into(),
        };
        let u = authorize_url(&cfg, "google", "http://127.0.0.1:47821/callback", "CH");
        assert!(u.starts_with("https://x.supabase.co/auth/v1/authorize?provider=google"));
        assert!(u.contains("redirect_to=http%3A%2F%2F127.0.0.1%3A47821%2Fcallback"));
        assert!(u.contains("code_challenge=CH&code_challenge_method=s256"));
    }

    #[test]
    fn parse_callback_reads_code() {
        assert_eq!(
            parse_callback("GET /callback?code=abc&state=x HTTP/1.1"),
            Ok("abc".to_string())
        );
    }

    #[test]
    fn parse_callback_reports_error() {
        let e = parse_callback(
            "GET /callback?error=access_denied&error_description=User+denied HTTP/1.1",
        )
        .unwrap_err();
        assert!(e.contains("User denied"));
    }

    #[test]
    fn parse_callback_needs_code() {
        assert!(parse_callback("GET /callback HTTP/1.1").is_err());
    }

    #[test]
    fn http_response_is_well_formed() {
        let r = http_response("200 OK", "hi");
        assert!(r.starts_with("HTTP/1.1 200 OK\r\nContent-Type: text/html"));
        assert_eq!(r.matches("\r\n\r\n").count(), 1);
        let (head, body) = r.split_once("\r\n\r\n").unwrap();
        assert!(body.starts_with("<!doctype"));
        assert!(head.contains(&format!("Content-Length: {}\r\n", body.len())));
    }

    #[test]
    fn parse_callback_strips_control_characters() {
        let e = parse_callback("GET /callback?error_description=%1B%5B31mX HTTP/1.1").unwrap_err();
        assert!(!e.contains('\u{1b}'));
        assert!(e.starts_with("The sign-in failed: "));
        let long = format!(
            "GET /callback?error_description={} HTTP/1.1",
            "a".repeat(500)
        );
        assert!(parse_callback(&long).unwrap_err().len() < 230);
    }

    struct Cancelling;

    impl Prompt for Cancelling {
        fn new_password(&mut self) -> Result<String, String> {
            Err("not used".to_string())
        }
        fn password(&mut self) -> Result<String, String> {
            Err("not used".to_string())
        }
        fn recovery_code(&mut self) -> Result<String, String> {
            Err("not used".to_string())
        }
        fn show_recovery_code(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn cancelled(&self) -> bool {
            true
        }
    }

    #[test]
    fn cancelled_sign_in_stops() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let started = Instant::now();
        let result = wait_for_code(&listener, &Cancelling, Instant::now() + WAIT);
        assert_eq!(result, Err("Sign-in was cancelled.".to_string()));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn only_get_is_accepted() {
        assert!(is_get("GET /callback HTTP/1.1"));
        assert!(!is_get("POST /callback HTTP/1.1"));
    }
}
