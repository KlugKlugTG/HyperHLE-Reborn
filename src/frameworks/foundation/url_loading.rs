/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Shared HTTP/HTTPS transport for Foundation's URL-loading APIs.
//!
//! The guest runtime is single-threaded and its Objective-C objects/guest
//! memory must never be touched from a host worker. Requests are therefore
//! copied into plain Rust data before dispatch; asynchronous callers receive
//! the result through a channel and deliver callbacks on the guest run loop.

use crate::frameworks::foundation::ns_string::{from_rust_string, to_rust_string};
use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::mem::MutPtr;
use crate::mem::MutVoidPtr;
use crate::objc::{autorelease, id, msg, msg_class, nil};
use crate::Environment;
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

const MAX_REQUEST_BODY_SIZE: usize = 64 * 1024 * 1024;
const MAX_RESPONSE_BODY_SIZE: usize = 256 * 1024 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 16;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_TIMEOUT: Duration = Duration::from_secs(300);

static ACTIVE_REQUESTS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout: Duration,
}

impl HttpRequest {
    pub fn get(url: String) -> Self {
        Self {
            url,
            method: "GET".to_string(),
            headers: Vec::new(),
            body: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub url: String,
    pub status_code: NSInteger,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct HttpError {
    /// The corresponding `NSURLError*` code.
    pub code: NSInteger,
    pub description: String,
}

pub type HttpResult = Result<HttpResponse, HttpError>;
pub type HttpReceiver = Receiver<HttpResult>;

fn http_error(code: NSInteger, description: impl Into<String>) -> HttpError {
    HttpError {
        code,
        description: description.into(),
    }
}

fn normalized_timeout(seconds: f64) -> Duration {
    if !seconds.is_finite() || seconds <= 0.0 {
        return DEFAULT_TIMEOUT;
    }
    Duration::from_secs_f64(seconds.clamp(0.1, MAX_TIMEOUT.as_secs_f64()))
}

fn headers_from_dictionary(env: &mut Environment, dictionary: id) -> Vec<(String, String)> {
    if dictionary == nil {
        return Vec::new();
    }
    let keys: id = msg![env; dictionary allKeys];
    if keys == nil {
        return Vec::new();
    }
    let count: NSUInteger = msg![env; keys count];
    let mut result = Vec::with_capacity(count.min(256) as usize);
    for index in 0..count.min(256) {
        let key: id = msg![env; keys objectAtIndex:index];
        let value: id = msg![env; dictionary objectForKey:key];
        if key == nil || value == nil {
            continue;
        }
        let name = to_rust_string(env, key).into_owned();
        let value_description: id = msg![env; value description];
        let value = to_rust_string(env, value_description).into_owned();
        if !name.is_empty() {
            result.push((name, value));
        }
    }
    result
}

fn read_stream_body(env: &mut Environment, stream: id) -> Result<Vec<u8>, HttpError> {
    const CHUNK_SIZE: NSUInteger = 16 * 1024;
    let mut body = Vec::new();
    () = msg![env; stream open];
    loop {
        let buffer: MutPtr<u8> = env.mem.alloc(CHUNK_SIZE).cast();
        let read: NSInteger = msg![env; stream read:buffer maxLength:CHUNK_SIZE];
        if read <= 0 {
            env.mem.free(buffer.cast());
            if read < 0 {
                () = msg![env; stream close];
                return Err(http_error(
                    -1005,
                    "The request body stream could not be read.",
                ));
            }
            break;
        }
        let chunk = env
            .mem
            .bytes_at(buffer.cast_const(), read as NSUInteger)
            .to_vec();
        env.mem.free(buffer.cast());
        if body.len().saturating_add(chunk.len()) > MAX_REQUEST_BODY_SIZE {
            () = msg![env; stream close];
            return Err(http_error(
                -1103,
                "The request body exceeds touchHLE's 64 MiB safety limit.",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    () = msg![env; stream close];
    Ok(body)
}

/// Copy an `NSURLRequest` and optional session defaults into host-owned data.
pub fn request_from_objc(
    env: &mut Environment,
    request: id,
    additional_headers: id,
    timeout_override: Option<f64>,
) -> Result<HttpRequest, HttpError> {
    if request == nil {
        return Err(http_error(-1000, "The URL request is nil."));
    }
    let url_object: id = msg![env; request URL];
    if url_object == nil {
        return Err(http_error(-1000, "The URL request has no URL."));
    }
    let absolute_string: id = msg![env; url_object absoluteString];
    if absolute_string == nil {
        return Err(http_error(-1000, "The URL request has an invalid URL."));
    }
    let url = to_rust_string(env, absolute_string).into_owned();

    let method_object: id = msg![env; request HTTPMethod];
    let method = if method_object == nil {
        "GET".to_string()
    } else {
        to_rust_string(env, method_object).into_owned()
    };
    if method.is_empty() {
        return Err(http_error(-1000, "The HTTP method is empty."));
    }

    let request_headers: id = msg![env; request allHTTPHeaderFields];
    let mut headers = headers_from_dictionary(env, request_headers);
    for (name, value) in headers_from_dictionary(env, additional_headers) {
        if !headers
            .iter()
            .any(|(existing, _)| existing.eq_ignore_ascii_case(&name))
        {
            headers.push((name, value));
        }
    }

    let body_data: id = msg![env; request HTTPBody];
    let body = if body_data != nil {
        let length: NSUInteger = msg![env; body_data length];
        if length as usize > MAX_REQUEST_BODY_SIZE {
            return Err(http_error(
                -1103,
                "The request body exceeds touchHLE's 64 MiB safety limit.",
            ));
        }
        if length == 0 {
            Vec::new()
        } else {
            let bytes: crate::mem::ConstVoidPtr = msg![env; body_data bytes];
            env.mem.bytes_at(bytes.cast(), length).to_vec()
        }
    } else {
        let body_stream: id = msg![env; request HTTPBodyStream];
        if body_stream == nil {
            Vec::new()
        } else {
            read_stream_body(env, body_stream)?
        }
    };

    let timeout = timeout_override.unwrap_or_else(|| msg![env; request timeoutInterval]);
    Ok(HttpRequest {
        url,
        method,
        headers,
        body,
        timeout: normalized_timeout(timeout),
    })
}

/// Copy a URL into a simple GET request (used by `NSData` URL initializers).
pub fn get_request_from_url(env: &mut Environment, url: id) -> Result<HttpRequest, HttpError> {
    if url == nil {
        return Err(http_error(-1000, "The URL is nil."));
    }
    let absolute_string: id = msg![env; url absoluteString];
    if absolute_string == nil {
        return Err(http_error(-1000, "The URL is invalid."));
    }
    Ok(HttpRequest::get(
        to_rust_string(env, absolute_string).into_owned(),
    ))
}

fn validate_request(request: &HttpRequest) -> Result<(), HttpError> {
    let Some((scheme, rest)) = request.url.split_once("://") else {
        return Err(http_error(-1000, "The URL is malformed."));
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(http_error(
            -1002,
            format!("Unsupported URL scheme in {:?}.", request.url),
        ));
    }
    if rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('#') {
        return Err(http_error(-1000, "The URL does not contain a host."));
    }
    if !request
        .method
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
    {
        return Err(http_error(-1000, "The HTTP method is not a valid token."));
    }
    Ok(())
}

fn transport_error(error: ureq::Error) -> HttpError {
    let text = match error {
        ureq::Error::Status(_, response) => {
            return http_error(
                -1011,
                format!(
                    "The server returned HTTP status {} {}.",
                    response.status(),
                    response.status_text()
                ),
            );
        }
        ureq::Error::Transport(error) => error.to_string(),
    };
    let lower = text.to_ascii_lowercase();
    let code = if lower.contains("timed out") || lower.contains("timeout") {
        -1001 // NSURLErrorTimedOut
    } else if lower.contains("certificate") {
        -1202 // NSURLErrorServerCertificateUntrusted
    } else if lower.contains("tls") || lower.contains("ssl") {
        -1200 // NSURLErrorSecureConnectionFailed
    } else if lower.contains("dns") || lower.contains("resolve") || lower.contains("name") {
        -1003 // NSURLErrorCannotFindHost
    } else if lower.contains("connect") || lower.contains("refused") {
        -1004 // NSURLErrorCannotConnectToHost
    } else {
        -1005 // NSURLErrorNetworkConnectionLost
    };
    http_error(code, format!("The HTTP request failed: {text}"))
}

fn response_from_ureq(response: ureq::Response) -> HttpResult {
    let status_code = response.status() as NSInteger;
    let response_url = response.get_url().to_string();
    let mut headers = Vec::new();
    for name in response.headers_names() {
        if let Some(value) = response.header(&name) {
            headers.push((canonical_header_name(&name), value.to_string()));
        }
    }
    let mut body = Vec::new();
    let mut reader = response
        .into_reader()
        .take((MAX_RESPONSE_BODY_SIZE + 1) as u64);
    reader.read_to_end(&mut body).map_err(|error| {
        http_error(
            -1005,
            format!("The HTTP response body could not be read: {error}"),
        )
    })?;
    if body.len() > MAX_RESPONSE_BODY_SIZE {
        return Err(http_error(
            -1103,
            "The response exceeds touchHLE's 256 MiB safety limit.",
        ));
    }
    Ok(HttpResponse {
        url: response_url,
        status_code,
        headers,
        body,
    })
}

fn canonical_header_name(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut canonical = first.to_ascii_uppercase().to_string();
                    canonical.push_str(&chars.as_str().to_ascii_lowercase());
                    canonical
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn perform_http_request(request: HttpRequest) -> HttpResult {
    validate_request(&request)?;
    let timeout = request
        .timeout
        .clamp(Duration::from_millis(100), MAX_TIMEOUT);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .redirects(10)
        .build();
    let mut outgoing = agent.request(&request.method, &request.url);
    for (name, value) in &request.headers {
        outgoing = outgoing.set(name, value);
    }
    let result = if request.body.is_empty() {
        outgoing.call()
    } else {
        outgoing.send_bytes(&request.body)
    };
    let response = match result {
        Ok(response) => response,
        // ureq treats non-2xx HTTP responses as errors by default. The URL
        // loading system must still return their body and HTTP response; only
        // transport failures are NSError failures.
        Err(ureq::Error::Status(_, response)) => response,
        Err(error) => return Err(transport_error(error)),
    };
    response_from_ureq(response)
}

fn network_disabled_error() -> HttpError {
    http_error(
        -1009,
        "Network access is disabled for this app. Enable --allow-network-access to connect.",
    )
}

/// Execute a request on the current guest thread (for synchronous Foundation
/// APIs). As with the real synchronous APIs, this blocks until the response or
/// timeout is received.
pub fn send(request: Result<HttpRequest, HttpError>, network_access: bool) -> HttpResult {
    let request = request?;
    if !network_access {
        return Err(network_disabled_error());
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        perform_http_request(request)
    })) {
        Ok(result) => result,
        Err(_) => Err(http_error(
            -1005,
            "The HTTP request failed unexpectedly in the host transport.",
        )),
    }
}

/// Execute a request away from the guest thread. Callbacks must still be
/// delivered by the caller on the guest/run-loop thread after receiving this
/// channel's result.
pub fn send_async(request: Result<HttpRequest, HttpError>, network_access: bool) -> HttpReceiver {
    let (sender, receiver) = mpsc::channel();
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            let _ = sender.send(Err(error));
            return receiver;
        }
    };
    if !network_access {
        let _ = sender.send(Err(network_disabled_error()));
        return receiver;
    }

    let mut active = ACTIVE_REQUESTS.load(Ordering::Relaxed);
    loop {
        if active >= MAX_CONCURRENT_REQUESTS {
            let _ = sender.send(Err(http_error(
                -1001,
                "Too many concurrent HTTP requests are active in touchHLE.",
            )));
            return receiver;
        }
        match ACTIVE_REQUESTS.compare_exchange_weak(
            active,
            active + 1,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(current) => active = current,
        }
    }

    let worker_sender = sender.clone();
    let spawn_result = std::thread::Builder::new()
        .name("HyperHLE HTTP request".to_string())
        .spawn(move || {
            let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                perform_http_request(request)
            })) {
                Ok(result) => result,
                Err(_) => Err(http_error(
                    -1005,
                    "The HTTP request failed unexpectedly in the host transport.",
                )),
            };
            ACTIVE_REQUESTS.fetch_sub(1, Ordering::AcqRel);
            let _ = worker_sender.send(result);
        });
    if let Err(error) = spawn_result {
        ACTIVE_REQUESTS.fetch_sub(1, Ordering::AcqRel);
        let _ = sender.send(Err(http_error(
            -1005,
            format!("Could not start the HTTP worker thread: {error}"),
        )));
    }
    receiver
}

/// Convert an HTTP response into an autoreleased `NSHTTPURLResponse`.
pub fn make_foundation_http_response(env: &mut Environment, response: &HttpResponse) -> id {
    let url_string = from_rust_string(env, response.url.clone());
    autorelease(env, url_string);
    let url: id = msg_class![env; NSURL URLWithString:url_string];

    let headers: id = msg_class![env; NSMutableDictionary new];
    autorelease(env, headers);
    for (name, value) in &response.headers {
        let key = from_rust_string(env, name.clone());
        autorelease(env, key);
        let value = from_rust_string(env, value.clone());
        autorelease(env, value);
        () = msg![env; headers setObject:value forKey:key];
    }

    let version = from_rust_string(env, "HTTP/1.1".to_string());
    autorelease(env, version);
    let http_response: id = msg_class![env; NSHTTPURLResponse alloc];
    let http_response: id = msg![env;
        http_response initWithURL:url
                       statusCode:(response.status_code)
                      HTTPVersion:version
                     headerFields:headers];
    autorelease(env, http_response)
}

/// Convert an HTTP response into autoreleased Foundation objects, as
/// URL-loading callbacks expect.
pub fn make_foundation_response(env: &mut Environment, response: &HttpResponse) -> (id, id) {
    let http_response = make_foundation_http_response(env, response);
    let data: id = if response.body.is_empty() {
        msg_class![env; NSData data]
    } else {
        let length: NSUInteger = response.body.len().try_into().unwrap_or(u32::MAX);
        let bytes: MutPtr<u8> = env.mem.alloc(length).cast();
        env.mem
            .bytes_at_mut(bytes, length)
            .copy_from_slice(&response.body);
        let data: id = msg_class![env;
            NSData dataWithBytes:(bytes.cast_const().cast_void()) length:length];
        env.mem.free(bytes.cast());
        data
    };
    (http_response, data)
}

/// Convert an `NSURLErrorDomain` failure into an autoreleased NSError.
pub fn make_foundation_error(env: &mut Environment, error: &HttpError) -> id {
    let domain = from_rust_string(env, "NSURLErrorDomain".to_string());
    autorelease(env, domain);
    let description_key =
        crate::frameworks::foundation::ns_string::get_static_str(env, "NSLocalizedDescription");
    let description = from_rust_string(env, error.description.clone());
    autorelease(env, description);
    let user_info: id = msg_class![env; NSMutableDictionary new];
    autorelease(env, user_info);
    () = msg![env; user_info setObject:description forKey:description_key];

    let ns_error: id = msg_class![env; NSError alloc];
    let ns_error: id = msg![env;
        ns_error initWithDomain:domain
                           code:(error.code)
                       userInfo:user_info];
    autorelease(env, ns_error)
}

/// Read a local `file://` `NSURLRequest` on the guest thread without requiring
/// `--allow-network-access` (used by Unity `WWW` and local asset loaders).
pub fn try_local_file_request(env: &mut Environment, request: id) -> Option<HttpResult> {
    if request == nil {
        return None;
    }
    let url_object: id = msg![env; request URL];
    if url_object == nil {
        return None;
    }
    let is_file: bool = msg![env; url_object isFileURL];
    if !is_file {
        return None;
    }
    let absolute_string: id = msg![env; url_object absoluteString];
    let url = if absolute_string == nil {
        String::new()
    } else {
        to_rust_string(env, absolute_string).into_owned()
    };
    let path: id = msg![env; url_object path];
    if path == nil {
        return Some(Err(http_error(-1100, "The file URL has no path.")));
    }
    let path_str = to_rust_string(env, path).into_owned();
    match env.fs.read(crate::fs::GuestPath::new(&path_str)) {
        Ok(body) => {
            let headers = vec![("Content-Length".to_string(), body.len().to_string())];
            Some(Ok(HttpResponse {
                url,
                status_code: 200,
                headers,
                body,
            }))
        }
        Err(_) => Some(Err(http_error(
            -1100,
            format!("The requested file {:?} could not be read.", path_str),
        ))),
    }
}

/// Wrap an already-computed `HttpResult` into a single-shot channel for async
/// run-loop callback delivery.
pub fn immediate_async_result(result: HttpResult) -> HttpReceiver {
    let (sender, receiver) = mpsc::channel();
    let _ = sender.send(result);
    receiver
}

/// Convenience for `NSError **` URL-loading out-parameters.
pub fn write_error_out_parameter(env: &mut Environment, error_ptr: MutVoidPtr, error: id) {
    if !error_ptr.is_null() {
        // `make_foundation_error` already returns an autoreleased NSError,
        // matching Objective-C's `__autoreleasing NSError **` convention.
        env.mem.write(error_ptr.cast(), error);
    }
}

#[cfg(test)]
mod tests {
    use super::{send, send_async, HttpRequest};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    fn serve_one(response: &'static [u8]) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0; 1];
            while !request.ends_with(b"\r\n\r\n") {
                if stream.read_exact(&mut byte).is_err() {
                    return;
                }
                request.push(byte[0]);
            }
            let _ = stream.write_all(response);
        });
        format!("http://{address}/test")
    }

    #[test]
    fn executes_http_get_and_preserves_response_body() {
        let url = serve_one(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 8\r\nConnection: close\r\n\r\n{\"ok\":1}",
        );
        let response = send(Ok(HttpRequest::get(url)), true).unwrap();
        assert_eq!(response.status_code, 200);
        assert_eq!(response.body, br#"{"ok":1}"#.to_vec());
        assert!(response
            .headers
            .iter()
            .any(|(name, value)| name == "Content-Type" && value == "application/json"));
    }

    #[test]
    fn non_success_http_status_is_a_response_not_a_transport_error() {
        let url = serve_one(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found",
        );
        let response = send(Ok(HttpRequest::get(url)), true).unwrap();
        assert_eq!(response.status_code, 404);
        assert_eq!(response.body, b"not found".to_vec());
    }

    #[test]
    fn asynchronous_requests_return_worker_results() {
        let url = serve_one(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        let response = send_async(Ok(HttpRequest::get(url)), true)
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(response.status_code, 200);
        assert_eq!(response.body, b"ok".to_vec());
    }

    #[test]
    fn disabled_access_returns_the_documented_url_error() {
        let error = send(Ok(HttpRequest::get("http://127.0.0.1/".to_string())), false).unwrap_err();
        assert_eq!(error.code, -1009);
        assert!(error.description.contains("--allow-network-access"));
    }

    #[test]
    fn rejects_non_http_schemes_before_opening_a_connection() {
        let error = send(
            Ok(HttpRequest::get("ftp://example.invalid/file".to_string())),
            true,
        )
        .unwrap_err();
        assert_eq!(error.code, -1002);
    }
}
