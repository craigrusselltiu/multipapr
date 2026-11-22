use crate::desktop::wide;
use anyhow::{Context, Result, anyhow, ensure};
use std::{
    ffi::c_void,
    io::{self, Read},
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError},
    Networking::WinHttp::*,
};

fn parse_https_url(url: &str) -> Result<(String, u16, String)> {
    let url = url::Url::parse(url).context("Invalid HTTP request URL")?;
    ensure!(
        url.scheme() == "https" && url.username().is_empty() && url.password().is_none(),
        "Request URL must use HTTPS without credentials"
    );
    let host = url.host_str().context("Missing host in request URL")?;
    let port = url.port().unwrap_or(INTERNET_DEFAULT_HTTPS_PORT);
    let path = &url[url::Position::BeforePath..url::Position::AfterQuery];
    Ok((host.to_owned(), port, path.to_owned()))
}

struct HttpHandle(*mut c_void);

impl HttpHandle {
    fn checked(ptr: *mut c_void, context: &str) -> Result<Self> {
        if ptr.is_null() {
            Err(anyhow!("{context}: {}", io::Error::last_os_error()))
        } else {
            Ok(Self(ptr))
        }
    }
}

impl Drop for HttpHandle {
    fn drop(&mut self) {
        unsafe {
            WinHttpCloseHandle(self.0);
        }
    }
}

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn check(&self) -> io::Result<()> {
        if self.0.load(Ordering::Relaxed) {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Request cancelled",
            ))
        } else {
            Ok(())
        }
    }
}

pub struct HttpRequest {
    request: HttpHandle,
    _connection: HttpHandle,
    _session: HttpHandle,
    cancellation: Option<Cancellation>,
    pub status: u32,
}

impl HttpRequest {
    pub fn send(
        verb: &str,
        url: &str,
        headers: &[&str],
        follow_redirects: bool,
        receive_timeout_ms: i32,
    ) -> Result<Self> {
        Self::send_request(
            verb,
            url,
            headers,
            follow_redirects,
            receive_timeout_ms,
            None,
        )
    }

    pub fn get_cancellable(
        url: &str,
        timeout_ms: i32,
        cancellation: &Cancellation,
    ) -> Result<Self> {
        Self::send_request("GET", url, &[], false, timeout_ms, Some(cancellation))
    }

    fn send_request(
        verb: &str,
        url: &str,
        headers: &[&str],
        follow_redirects: bool,
        receive_timeout_ms: i32,
        cancellation: Option<&Cancellation>,
    ) -> Result<Self> {
        let check = || cancellation.map_or(Ok(()), Cancellation::check);
        check()?;
        let (host, port, path) = parse_https_url(url)?;
        let agent = wide(concat!(
            "multipapr/",
            env!("CARGO_PKG_VERSION"),
            " (https://github.com/craigrusselltiu/multipapr)"
        ));
        let session = HttpHandle::checked(
            unsafe {
                let h = WinHttpOpen(
                    agent.as_ptr(),
                    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                    null(),
                    null(),
                    0,
                );
                if h.is_null() {
                    WinHttpOpen(
                        agent.as_ptr(),
                        WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
                        null(),
                        null(),
                        0,
                    )
                } else {
                    h
                }
            },
            "Cannot initialise HTTP client",
        )?;
        let connection = HttpHandle::checked(
            unsafe { WinHttpConnect(session.0, wide(&host).as_ptr(), port, 0) },
            "Cannot connect to the server",
        )?;
        let request = HttpHandle::checked(
            unsafe {
                WinHttpOpenRequest(
                    connection.0,
                    wide(verb).as_ptr(),
                    wide(&path).as_ptr(),
                    null(),
                    null(),
                    null(),
                    WINHTTP_FLAG_SECURE,
                )
            },
            "Cannot create HTTP request",
        )?;
        unsafe {
            WinHttpSetTimeouts(request.0, 10_000, 10_000, 15_000, receive_timeout_ms);
            let policy = if follow_redirects {
                WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP
            } else {
                WINHTTP_OPTION_REDIRECT_POLICY_NEVER
            };
            ensure!(
                WinHttpSetOption(
                    request.0,
                    WINHTTP_OPTION_REDIRECT_POLICY,
                    (&policy as *const u32).cast(),
                    size_of::<u32>() as u32,
                ) != 0,
                "Cannot set HTTP redirect policy"
            );
            for header in headers {
                let wide_header = wide(header);
                ensure!(
                    WinHttpAddRequestHeaders(
                        request.0,
                        wide_header.as_ptr(),
                        u32::MAX,
                        WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE,
                    ) != 0,
                    "Cannot set HTTP request header"
                );
            }
            check()?;
            if WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) == 0 {
                return Err(io::Error::last_os_error())
                    .with_context(|| format!("Cannot reach {host}"));
            }
            check()?;
            if WinHttpReceiveResponse(request.0, null_mut()) == 0 {
                return Err(io::Error::last_os_error())
                    .with_context(|| format!("No response from {host}"));
            }
            check()?;
            let mut status: u32 = 0;
            let mut len = size_of::<u32>() as u32;
            ensure!(
                WinHttpQueryHeaders(
                    request.0,
                    WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                    null(),
                    (&mut status as *mut u32).cast(),
                    &mut len,
                    null_mut(),
                ) != 0,
                "Cannot read HTTP response status"
            );
            Ok(Self {
                _session: session,
                _connection: connection,
                request,
                cancellation: cancellation.cloned(),
                status,
            })
        }
    }

    pub fn content_length(&self) -> Option<u64> {
        let mut value: u64 = 0;
        let mut len = size_of::<u64>() as u32;
        let ok = unsafe {
            WinHttpQueryHeaders(
                self.request.0,
                WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER64,
                null(),
                (&mut value as *mut u64).cast(),
                &mut len,
                null_mut(),
            ) != 0
        };
        (ok && value > 0).then_some(value)
    }

    pub fn header_string(&self, level: u32) -> Option<String> {
        let mut byte_len: u32 = 0;
        unsafe {
            let ok = WinHttpQueryHeaders(
                self.request.0,
                level,
                null(),
                null_mut(),
                &mut byte_len,
                null_mut(),
            );
            if ok != 0 || GetLastError() != ERROR_INSUFFICIENT_BUFFER || byte_len == 0 {
                return None;
            }
            let mut buf = vec![0u16; (byte_len as usize).div_ceil(2)];
            if WinHttpQueryHeaders(
                self.request.0,
                level,
                null(),
                buf.as_mut_ptr().cast(),
                &mut byte_len,
                null_mut(),
            ) == 0
            {
                return None;
            }
            let units = (byte_len as usize / 2).min(buf.len());
            Some(
                String::from_utf16_lossy(&buf[..units])
                    .trim_end_matches('\0')
                    .to_string(),
            )
        }
    }

    pub fn read_all(self, max_bytes: usize) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.take(max_bytes as u64 + 1)
            .read_to_end(&mut out)
            .context("Cannot read HTTP response")?;
        ensure!(out.len() <= max_bytes, "HTTP response is too large");
        Ok(out)
    }
}

impl Read for HttpRequest {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let check = || {
            self.cancellation
                .as_ref()
                .map_or(Ok(()), Cancellation::check)
        };
        check()?;
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut read = 0;
        let len = buffer.len().min(u32::MAX as usize) as u32;
        let ok =
            unsafe { WinHttpReadData(self.request.0, buffer.as_mut_ptr().cast(), len, &mut read) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        check()?;
        Ok(read as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_browsing_stops_before_http_dispatch() {
        let cancellation = Cancellation::default();
        let request = cancellation.clone();
        cancellation.cancel();
        let error = HttpRequest::get_cancellable("not a URL", 1, &request)
            .err()
            .expect("cancelled request");
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::ConnectionAborted,
        );
    }

    #[test]
    fn parses_https_urls_only() {
        assert_eq!(
            parse_https_url("https://api.github.com/repos/a/b").unwrap(),
            ("api.github.com".into(), 443, "/repos/a/b".into())
        );
        assert_eq!(
            parse_https_url("https://example.com:8443/x?y=1").unwrap(),
            ("example.com".into(), 8443, "/x?y=1".into())
        );
        assert!(parse_https_url("http://example.com/a").is_err());
    }
}
