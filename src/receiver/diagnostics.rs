//! Authentication metadata only. Never pass bodies, errors or arbitrary headers
//! to tracing. Receiver-controlled values must pass a value allowlist as well.
pub(super) fn metadata(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || ".,_-".contains(*c))
        .take(64)
        .collect()
}
fn safe_header(name: &str, value: &str) -> Option<(&'static str, String)> {
    let name = name.to_ascii_lowercase();
    let value = value.trim();
    let canonical = match name.as_str() {
        "content-length" | "cseq" | "x-apple-hkp"
            if !value.is_empty()
                && value.len() <= 10
                && value.bytes().all(|b| b.is_ascii_digit()) =>
        {
            match name.as_str() {
                "content-length" => "content-length",
                "cseq" => "cseq",
                _ => "x-apple-hkp",
            }
        }
        "connection" if ["close", "keep-alive"].contains(&value.to_ascii_lowercase().as_str()) => {
            "connection"
        }
        "content-type"
            if [
                "application/octet-stream",
                "application/x-apple-binary-plist",
            ]
            .contains(&value) =>
        {
            "content-type"
        }
        "user-agent" if ["PiAir/0.1", "AirPlay/320.20"].contains(&value) => "user-agent",
        // Host is emitted by our constructor; omit receiver Host and arbitrary strings.
        _ => return None,
    };
    Some((canonical, value.to_owned()))
}
fn headers(text: &str) -> Vec<(&'static str, String)> {
    text.split("\r\n")
        .skip(1)
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            safe_header(name, value)
        })
        .collect()
}
fn reason(status: &str, supplied: &str) -> &'static str {
    // Do not echo arbitrary reason text: it can contain tokens or reflected secrets.
    match (status, supplied) {
        ("200", "OK") => "OK",
        ("403", "Forbidden") => "Forbidden",
        ("470", "Connection Authorization Required") => "Connection Authorization Required",
        _ => "unrecognized (omitted)",
    }
}
pub(super) fn request(header: &str, body_length: usize, phase: &str) {
    let line = header.lines().next().unwrap_or_default();
    // Only called after the endpoint allowlist in Wire::post_inner.
    tracing::debug!(target: "piair::auth", phase, request = line, body_length,
        headers = ?headers(header), "authentication request");
}
pub(super) fn response(text: &str, phase: &str) {
    let mut words = text.lines().next().unwrap_or_default().splitn(3, ' ');
    let protocol = words.next().unwrap_or_default();
    let status = words.next().unwrap_or_default();
    let reason = reason(status, words.next().unwrap_or_default());
    tracing::debug!(target: "piair::auth", phase, protocol, status, reason,
        headers = ?headers(text), "authentication response");
}

#[cfg(test)]
pub(super) mod tests {
    use std::sync::{Arc, Mutex};
    #[derive(Clone)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    pub(in crate::receiver) fn capture(level: tracing::Level, run: impl FnOnce()) -> String {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let sink = Sink(bytes.clone());
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_max_level(level)
            .with_writer(move || sink.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, run);
        let output = bytes.lock().unwrap().clone();
        String::from_utf8(output).unwrap()
    }
    #[test]
    fn emitted_output_is_opt_in_and_never_contains_payloads_or_reflected_secrets() {
        let emit = || {
            request(
                "POST /pair-setup HTTP/1.1\r\nX-Apple-HKP: 4\r\nContent-Length: 9\r\n\r\n",
                9,
                "pair-setup M1/M2",
            );
            response("HTTP/1.1 403 secret-proof\r\nSet-Cookie: secret-cookie\r\nAuthorization: secret-auth\r\nX-Unknown: secret-token\r\nCSeq: 1\r\n\r\n", "pair-setup M1/M2");
        };
        assert!(capture(tracing::Level::INFO, emit).is_empty());
        let output = capture(tracing::Level::DEBUG, emit);
        for field in [
            "403",
            "HTTP/1.1",
            "pair-setup M1/M2",
            "body_length",
            "x-apple-hkp",
        ] {
            assert!(output.contains(field), "missing {field}: {output}");
        }
        assert!(!output.contains("secret"));
    }

    use super::*;
    #[test]
    fn headers_are_explicitly_allowlisted_with_safe_values() {
        let text = "HTTP/1.1 403 Forbidden\r\nCSeq: 1\r\nContent-Length: 0\r\nConnection: close\r\nSet-Cookie: secret\r\nAuthorization: secret\r\nCookie: secret\r\nX-Token: secret\r\nX-Unknown: secret\r\nServer: secret\r\nContent-Type: secret\r\nX-Apple-HKP: secret\r\n\r\n";
        let output = format!("{:?}", headers(text));
        assert!(!output.contains("secret"));
        assert_eq!(
            headers(text),
            vec![
                ("cseq", "1".into()),
                ("content-length", "0".into()),
                ("connection", "close".into())
            ]
        );
        assert_eq!(reason("403", "Forbidden"), "Forbidden");
        assert_eq!(reason("403", "secret"), "unrecognized (omitted)");
        assert_eq!(safe_header("cSeQ", "2"), Some(("cseq", "2".into())));
        assert!(safe_header("Connection", "close\nsecret").is_none());
        assert!(safe_header("Content-Length", "12345678901").is_none());
    }
}
