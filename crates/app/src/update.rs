//! Updates from GitHub. When Flores PDF starts, if "Check for updates" is on, a thread asks GitHub
//! for the latest release of `REPO`; when it's newer than this copy, the toolbar offers it. For the
//! installed copy, Update downloads that release's `FloresPDF-Setup.exe` into the temp folder and
//! runs it quietly: it waits for this copy to close, puts the new one in its place and starts it. A
//! copy that wasn't installed opens the release's page instead.
//!
//! Requests go through WinHTTP, Windows' own, so the exe carries no TLS library. Nothing is sent but
//! the request itself (and the program's name and version, as its user agent).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle,
    WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetTimeouts,
};

/// Where releases are published.
pub const REPO: &str = "AgastyaShenoy/flores-pdf";
/// A download bigger than this isn't our setup.
const MAX_DOWNLOAD: usize = 64 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    /// The release's page on GitHub.
    pub page: String,
    /// Its setup, to download, if it has one.
    pub setup: Option<String>,
}

/// This copy's version. `FLORES_UPDATE_TEST_VERSION` stands in for it, to try updating.
pub fn current() -> String {
    std::env::var("FLORES_UPDATE_TEST_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_owned())
}

/// The latest release, if it's newer than this copy.
pub fn check() -> Result<Option<Release>, String> {
    let body = get(&format!("https://api.github.com/repos/{REPO}/releases/latest"), "application/vnd.github+json")?;
    let text = String::from_utf8_lossy(&body);
    let tag = json_strings(&text, "tag_name").into_iter().next().ok_or("GitHub's answer names no release")?;
    let version = tag.trim_start_matches('v').to_owned();
    let setup = json_strings(&text, "browser_download_url").into_iter().find(|u| u.ends_with("/FloresPDF-Setup.exe"));
    let page = format!("https://github.com/{REPO}/releases/tag/{tag}");
    Ok(newer(&version, &current()).then_some(Release { version, page, setup }))
}

/// Whether version `a` comes after `b`, number by number: 0.10.0 is after 0.9.2, and 1.0 is 1.0.0.
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| v.split(['.', '-', '+']).map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (mut a, mut b) = (parts(a), parts(b));
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

/// Whether this is the installed copy, the one setup replaces.
pub fn installed() -> bool {
    let (Ok(exe), Some(dir)) = (std::env::current_exe(), crate::install::install_dir()) else { return false };
    crate::history::same_file(&exe, &dir.join("FloresPDF.exe"))
}

/// Downloads the release's setup into the temp folder.
pub fn download(release: &Release) -> Result<PathBuf, String> {
    let url = release.setup.as_deref().ok_or("this release has no setup")?;
    let data = get(url, "application/octet-stream")?;
    if data.len() < 1 << 20 || !data.starts_with(b"MZ") {
        return Err("what came down isn't the setup".to_owned());
    }
    let path = std::env::temp_dir().join(format!("FloresPDF-Setup-{}.exe", release.version));
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Starts the setup, quietly: once this process has ended it installs, then starts the new copy.
pub fn install(setup: &Path) -> Result<(), String> {
    let pid = std::process::id().to_string();
    std::process::Command::new(setup).args(["/S", "--run", "--after", &pid]).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// An open WinHTTP handle, closed on drop.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a handle WinHTTP gave, closed once.
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// The body of `url` (https only), following redirects; an error unless the answer is 200 OK.
fn get(url: &str, accept: &str) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or("only https addresses are fetched")?;
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h, format!("/{p}")),
        None => (rest, "/".to_owned()),
    };
    let failed = |what: &str| format!("couldn't {what} {host} (error {})", std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
    let (agent, host_w, verb, path_w) = (wide(&format!("FloresPDF/{}", env!("CARGO_PKG_VERSION"))), wide(host), wide("GET"), wide(&path));
    let headers = wide(&format!("Accept: {accept}\r\n"));
    // SAFETY: NUL-terminated strings that outlive the calls; handles are checked before use and
    // closed by Handle; the buffers are as big as said.
    unsafe {
        let session = Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0));
        if session.0.is_null() {
            return Err(failed("start a connection to"));
        }
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 30_000, 60_000);
        let connect = Handle(WinHttpConnect(session.0, host_w.as_ptr(), 443, 0));
        if connect.0.is_null() {
            return Err(failed("connect to"));
        }
        let request = Handle(WinHttpOpenRequest(connect.0, verb.as_ptr(), path_w.as_ptr(), null(), null(), null(), WINHTTP_FLAG_SECURE));
        if request.0.is_null() || WinHttpSendRequest(request.0, headers.as_ptr(), u32::MAX, null(), 0, 0, 0) == 0 {
            return Err(failed("reach"));
        }
        if WinHttpReceiveResponse(request.0, null_mut()) == 0 {
            return Err(failed("hear back from"));
        }
        let (mut status, mut len) = (0u32, 4u32);
        let level = WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER;
        WinHttpQueryHeaders(request.0, level, null(), (&raw mut status).cast(), &mut len, null_mut());
        if status != 200 {
            return Err(format!("{host} answered {status}"));
        }
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            if WinHttpReadData(request.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut read) == 0 {
                return Err(failed("download from"));
            }
            if read == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..read as usize]);
            if out.len() > MAX_DOWNLOAD {
                return Err(format!("{host} sent more than expected"));
            }
        }
    }
}

/// The string values of `key` in JSON text, in order, with escapes decoded: enough for GitHub's
/// answer, without a JSON library.
fn json_strings(text: &str, key: &str) -> Vec<String> {
    let pattern = format!("\"{key}\"");
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(&pattern) {
        rest = &rest[i + pattern.len()..];
        let Some(value) = rest.trim_start().strip_prefix(':').map(str::trim_start) else { continue };
        let Some(body) = value.strip_prefix('"') else { continue };
        let mut s = String::new();
        let mut chars = body.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('u') => {
                        let hex: String = chars.by_ref().take(4).collect();
                        s.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
                    }
                    Some(c) => s.push(c),
                    None => break,
                },
                c => s.push(c),
            }
        }
        found.push(s);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.10.0", "0.9.2"));
        assert!(newer("1.0", "0.99.99"));
        assert!(!newer("1.0", "1.0.0"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(newer("0.1.1", "0.1.0"));
    }

    #[test]
    fn reads_github_release_fields() {
        let text = r#"{"html_url":"x","tag_name":"v0.2.0","assets":[{"name":"FloresPDF.exe","browser_download_url":"https://github.com/a/b/releases/download/v0.2.0/FloresPDF.exe"},{"browser_download_url" : "https:\/\/github.com\/a\/b\/releases\/download\/v0.2.0\/FloresPDF-Setup.exe"}],"body":"line\none é"}"#;
        assert_eq!(json_strings(text, "tag_name"), ["v0.2.0"]);
        let urls = json_strings(text, "browser_download_url");
        assert_eq!(urls[1], "https://github.com/a/b/releases/download/v0.2.0/FloresPDF-Setup.exe");
        assert_eq!(json_strings(text, "body"), ["line\none é"]);
    }
}
