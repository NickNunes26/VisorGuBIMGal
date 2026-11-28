//! Platform-independent pieces of the desktop shell (E9), unit-tested on any host.
//!
//! - which command-line arguments are `.ifc` files to open;
//! - the registry of files the page may read through `https://gubim-file.localhost/<token>`;
//! - the `WM_COPYDATA` payload a second instance sends to the first;
//! - the JSON messages between the page and the host;
//! - the query string of a save request.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Origin the app is served from (WebView2 virtual host mapped to the `app` folder).
pub const APP_HOST: &str = "app.gubim.localhost";
/// Host the page reads local IFC files from and posts saved files to.
pub const FILE_HOST: &str = "gubim-file.localhost";
/// `dwData` tag of the `WM_COPYDATA` a second instance sends ("GUBI").
pub const COPYDATA_OPEN_FILES: usize = 0x4755_4249;

pub fn app_origin() -> String {
    format!("https://{APP_HOST}")
}

/// A file the page may read, as the page sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileToken {
    pub token: String,
    pub name: String,
    pub size: u64,
}

pub fn is_ifc_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("ifc"))
}

/// The `.ifc` files among the arguments after the program name. Flags (`--x`) and the value
/// that follows a flag taking one are skipped; anything that is not an existing `.ifc` file
/// is ignored.
pub fn collect_ifc_args<I, S>(args: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = Vec::new();
    let mut skip_value = false;
    for raw in args.into_iter().skip(1) {
        let arg = raw.as_ref().trim().trim_matches('"');
        if skip_value {
            skip_value = false;
            continue;
        }
        if let Some(flag) = arg.strip_prefix("--") {
            skip_value = FLAGS_WITH_VALUE.contains(&flag) && !flag.contains('=');
            continue;
        }
        if arg.is_empty() || arg.starts_with('-') {
            continue;
        }
        let p = PathBuf::from(arg);
        if is_ifc_path(&p) && p.is_file() {
            out.push(p);
        }
    }
    out
}

/// Flags of this program that take a value in the next argument.
pub const FLAGS_WITH_VALUE: &[&str] = &["dev-url", "app-dir"];

/// Value of `--name value` or `--name=value`.
pub fn flag_value<S: AsRef<str>>(args: &[S], name: &str) -> Option<String> {
    let long = format!("--{name}");
    let eq = format!("--{name}=");
    let mut it = args.iter().map(|a| a.as_ref());
    while let Some(a) = it.next() {
        if a == long {
            return it.next().map(str::to_string);
        }
        if let Some(v) = a.strip_prefix(&eq) {
            return Some(v.to_string());
        }
    }
    None
}

/// Files the page has been allowed to read, by opaque token. Only files the user picked,
/// double-clicked or dropped get a token; the page cannot name an arbitrary path.
#[derive(Default)]
pub struct FileRegistry {
    next_id: u64,
    paths: HashMap<String, PathBuf>,
}

impl FileRegistry {
    pub fn register(&mut self, path: &Path) -> Option<FileToken> {
        let canonical = fs::canonicalize(path).ok()?;
        let meta = fs::metadata(&canonical).ok()?;
        if !meta.is_file() {
            return None;
        }
        let name = canonical
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("modelo.ifc")
            .to_string();
        self.next_id += 1;
        let token = format!("f-{}", self.next_id);
        self.paths.insert(token.clone(), canonical);
        Some(FileToken { token, name, size: meta.len() })
    }

    pub fn resolve(&self, token: &str) -> Option<&Path> {
        self.paths.get(token).map(PathBuf::as_path)
    }
}

/// Files waiting for the page: launch arguments and files that arrive before the page asks.
#[derive(Default)]
pub struct PendingFiles {
    queue: Vec<FileToken>,
    /// True once the current page has drained the queue; later files go out as events.
    drained: bool,
}

impl PendingFiles {
    /// Queues the file, or returns it if the page is ready to receive it as an event.
    pub fn offer(&mut self, file: FileToken) -> Option<FileToken> {
        if self.drained {
            Some(file)
        } else {
            self.queue.push(file);
            None
        }
    }

    pub fn take_all(&mut self) -> Vec<FileToken> {
        self.drained = true;
        std::mem::take(&mut self.queue)
    }

    /// A new page is loading: it has not subscribed yet.
    pub fn page_reloading(&mut self) {
        self.drained = false;
    }
}

/// `WM_COPYDATA` payload: absolute paths as UTF-16, each followed by a NUL.
pub fn encode_open_request(paths: &[PathBuf]) -> Vec<u16> {
    let mut out = Vec::new();
    for p in paths {
        out.extend(p.to_string_lossy().encode_utf16());
        out.push(0);
    }
    out
}

pub fn decode_open_request(data: &[u16]) -> Vec<PathBuf> {
    data.split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(|s| PathBuf::from(String::from_utf16_lossy(s)))
        .collect()
}

/// A request from the page: `{ id, cmd, args }`.
#[derive(Debug, Deserialize, PartialEq)]
pub struct HostRequest {
    pub id: u64,
    pub cmd: String,
    #[serde(default)]
    pub args: Value,
}

pub fn parse_request(json: &str) -> Option<HostRequest> {
    serde_json::from_str(json).ok()
}

pub fn reply_ok(id: u64, result: Value) -> String {
    serde_json::json!({ "id": id, "ok": true, "result": result }).to_string()
}

pub fn reply_err(id: u64, message: &str) -> String {
    serde_json::json!({ "id": id, "ok": false, "error": message }).to_string()
}

pub fn open_file_event(file: &FileToken) -> String {
    serde_json::json!({ "event": "open-file", "file": file }).to_string()
}

pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What the page asks for when it posts a file to save.
#[derive(Debug, PartialEq, Eq)]
pub struct SaveRequest {
    pub default_name: String,
    pub filter_name: String,
    pub extensions: Vec<String>,
}

/// Reads `name`, `filter` and `ext` (comma-separated) from a URL query string.
pub fn parse_save_query(query: &str) -> SaveRequest {
    let mut req = SaveRequest {
        default_name: String::new(),
        filter_name: String::new(),
        extensions: Vec::new(),
    };
    for pair in query.trim_start_matches('?').split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = percent_decode(v);
        match k {
            "name" => req.default_name = sanitize_file_name(&v),
            "filter" => req.filter_name = v,
            "ext" => {
                req.extensions = v
                    .split(',')
                    .map(|e| e.trim().trim_start_matches('.').to_string())
                    .filter(|e| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()))
                    .collect()
            }
            _ => {}
        }
    }
    req
}

/// Largest byte range one request may ask for. WebView2 copies a response several times inside
/// the host process before sending it on (measured: a 300 MB response peaked at 2.4 GB), so
/// files are read in ranges and the host's memory stays bounded whatever the file size (E4).
pub const MAX_RANGE: u64 = 16 << 20;

/// `start`/`end` (end exclusive) of a file-read URL's query, checked against the file size.
/// `Err` with the HTTP status when the range is missing, malformed, out of bounds or too large.
pub fn parse_range(url: &str, file_size: u64) -> Result<(u64, u64), u16> {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    let (mut start, mut end) = (None, None);
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("start", v)) => start = v.parse::<u64>().ok(),
            Some(("end", v)) => end = v.parse::<u64>().ok(),
            _ => {}
        }
    }
    let (Some(start), Some(end)) = (start, end) else { return Err(400) };
    if start > end || end > file_size {
        return Err(416);
    }
    if end - start > MAX_RANGE {
        return Err(413);
    }
    Ok((start, end))
}

/// A default file name the save dialog can show: no folders, no characters Windows refuses.
pub fn sanitize_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    base.chars()
        .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') && !c.is_control())
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string()
}

/// The token in a `https://gubim-file.localhost/<token>` URL, if that is what it is.
pub fn file_token_from_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix(&format!("https://{FILE_HOST}/"))?;
    let path = rest.split(['?', '#']).next()?;
    (!path.is_empty() && !path.contains('/')).then(|| percent_decode(path))
}

/// The query of a `https://gubim-file.localhost/__save?...` URL.
pub fn save_query_from_url(url: &str) -> Option<&str> {
    let rest = url.strip_prefix(&format!("https://{FILE_HOST}/__save"))?;
    Some(rest.strip_prefix('?').unwrap_or(rest))
}

/// May the main frame go to this URL? Only the app itself (or the dev server in dev mode);
/// anything else opens in the user's browser.
pub fn is_app_url(url: &str, dev_url: Option<&str>) -> bool {
    let app = format!("{}/", app_origin());
    if url.starts_with(&app) || url == app_origin() {
        return true;
    }
    if let Some(dev) = dev_url {
        let dev = dev.trim_end_matches('/');
        return url == dev || url.starts_with(&format!("{dev}/"));
    }
    false
}

pub fn is_external_web_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:")
}

/// `\\?\D:\x` -> `D:\x`; `\\?\UNC\server\share` -> `\\server\share`; anything else unchanged.
pub fn without_verbatim_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gubim-e9-{tag}-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn collect_ifc_args_skips_flags_flag_values_and_non_ifc() {
        let dir = temp_dir("args");
        let ifc = dir.join("Sample.IFC");
        let other = dir.join("b.ifc");
        let txt = dir.join("readme.txt");
        fs::write(&ifc, b"ISO-10303-21;").unwrap();
        fs::write(&other, b"ISO-10303-21;").unwrap();
        fs::write(&txt, b"x").unwrap();
        let missing = dir.join("missing.ifc");
        let args = vec![
            "app.exe".to_string(),
            "--dev-url".to_string(),
            other.to_string_lossy().into_owned(), // value of --dev-url, not a file to open
            "--app-dir=C:\\x".to_string(),
            txt.to_string_lossy().into_owned(),
            missing.to_string_lossy().into_owned(),
            format!("\"{}\"", ifc.to_string_lossy()),
        ];
        assert_eq!(collect_ifc_args(&args), vec![ifc.clone()]);
        assert_eq!(flag_value(&args, "dev-url").as_deref(), Some(other.to_str().unwrap()));
        assert_eq!(flag_value(&args, "app-dir").as_deref(), Some("C:\\x"));
        assert_eq!(flag_value(&args, "nope"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn registry_gives_tokens_only_for_existing_files() {
        let dir = temp_dir("reg");
        let ifc = dir.join("modelo planta 1.ifc");
        fs::write(&ifc, b"ISO-10303-21;").unwrap();
        let mut reg = FileRegistry::default();
        let a = reg.register(&ifc).expect("file");
        let b = reg.register(&ifc).expect("file");
        assert_eq!(a.name, "modelo planta 1.ifc");
        assert_eq!(a.size, 13);
        assert_ne!(a.token, b.token);
        assert_eq!(reg.resolve(&a.token), Some(fs::canonicalize(&ifc).unwrap().as_path()));
        assert!(reg.resolve("f-999").is_none());
        assert!(reg.register(&dir).is_none(), "a folder is not a file");
        assert!(reg.register(&dir.join("nope.ifc")).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_files_queue_until_drained_then_become_events_until_reload() {
        let tok = |n: &str| FileToken { token: n.into(), name: n.into(), size: 1 };
        let mut p = PendingFiles::default();
        assert_eq!(p.offer(tok("a")), None);
        assert_eq!(p.offer(tok("b")), None);
        assert_eq!(p.take_all(), vec![tok("a"), tok("b")]);
        assert_eq!(p.offer(tok("c")), Some(tok("c")));
        p.page_reloading();
        assert_eq!(p.offer(tok("d")), None);
        assert_eq!(p.take_all(), vec![tok("d")]);
    }

    #[test]
    fn copydata_payload_round_trips_unicode_paths() {
        let paths = vec![
            PathBuf::from(r"C:\Proyectos\Puente Ñandú\modelo.ifc"),
            PathBuf::from(r"D:\b.ifc"),
        ];
        let data = encode_open_request(&paths);
        assert_eq!(*data.last().unwrap(), 0);
        assert_eq!(decode_open_request(&data), paths);
        assert!(decode_open_request(&[]).is_empty());
    }

    #[test]
    fn requests_and_replies_are_json() {
        let r = parse_request(r#"{"id":7,"cmd":"registerIfcPath","args":{"path":"C:\\a.ifc"}}"#).unwrap();
        assert_eq!(r.id, 7);
        assert_eq!(r.cmd, "registerIfcPath");
        assert_eq!(r.args["path"], "C:\\a.ifc");
        assert_eq!(parse_request(r#"{"id":1,"cmd":"takePendingFiles"}"#).unwrap().args, Value::Null);
        assert!(parse_request("not json").is_none());
        let ok: Value = serde_json::from_str(&reply_ok(7, serde_json::json!([1]))).unwrap();
        assert_eq!(ok["ok"], true);
        assert_eq!(ok["result"][0], 1);
        let err: Value = serde_json::from_str(&reply_err(8, "fallo")).unwrap();
        assert_eq!(err["ok"], false);
        assert_eq!(err["error"], "fallo");
        let ev: Value = serde_json::from_str(&open_file_event(&FileToken {
            token: "f-1".into(),
            name: "a.ifc".into(),
            size: 5,
        }))
        .unwrap();
        assert_eq!(ev["event"], "open-file");
        assert_eq!(ev["file"]["token"], "f-1");
    }

    #[test]
    fn save_query_is_decoded_and_cleaned() {
        let q = parse_save_query("?name=..%2F..%2Fplano%20%C3%B1%3F.dxf&filter=Plano%20DXF&ext=dxf,.svg,b%2Fad,");
        assert_eq!(q.default_name, "plano ñ.dxf");
        assert_eq!(q.filter_name, "Plano DXF");
        assert_eq!(q.extensions, vec!["dxf", "svg"]);
        assert_eq!(parse_save_query(""), SaveRequest {
            default_name: String::new(),
            filter_name: String::new(),
            extensions: vec![]
        });
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("a%zz"), "a%zz");
    }

    #[test]
    fn ranges_are_checked_against_the_file_and_the_cap() {
        let url = |q: &str| format!("https://gubim-file.localhost/f-1?{q}");
        assert_eq!(parse_range(&url("start=0&end=10"), 10), Ok((0, 10)));
        assert_eq!(parse_range(&url("end=7&start=3"), 10), Ok((3, 7)));
        assert_eq!(parse_range(&url("start=5&end=5"), 10), Ok((5, 5)));
        assert_eq!(parse_range("https://gubim-file.localhost/f-1", 10), Err(400));
        assert_eq!(parse_range(&url("start=0"), 10), Err(400));
        assert_eq!(parse_range(&url("start=-1&end=3"), 10), Err(400));
        assert_eq!(parse_range(&url("start=6&end=5"), 10), Err(416));
        assert_eq!(parse_range(&url("start=0&end=11"), 10), Err(416));
        let big = MAX_RANGE * 4;
        assert_eq!(parse_range(&url(&format!("start=0&end={MAX_RANGE}")), big), Ok((0, MAX_RANGE)));
        assert_eq!(parse_range(&url(&format!("start=0&end={}", MAX_RANGE + 1)), big), Err(413));
    }

    #[test]
    fn urls_are_classified() {
        assert_eq!(file_token_from_url("https://gubim-file.localhost/f-3"), Some("f-3".into()));
        assert_eq!(file_token_from_url("https://gubim-file.localhost/f-3?x=1"), Some("f-3".into()));
        assert_eq!(file_token_from_url("https://gubim-file.localhost/a/b"), None);
        assert_eq!(file_token_from_url("https://gubim-file.localhost/"), None);
        assert_eq!(file_token_from_url("https://evil.localhost/f-3"), None);
        assert_eq!(save_query_from_url("https://gubim-file.localhost/__save?name=a"), Some("name=a"));
        assert_eq!(save_query_from_url("https://gubim-file.localhost/f-1"), None);

        assert!(is_app_url("https://app.gubim.localhost/index.html", None));
        assert!(is_app_url("https://app.gubim.localhost", None));
        assert!(!is_app_url("https://app.gubim.localhost.evil.com/", None));
        assert!(!is_app_url("http://localhost:5173/", None));
        assert!(is_app_url("http://localhost:5173/", Some("http://localhost:5173")));
        assert!(!is_app_url("http://localhost:51730/", Some("http://localhost:5173/")));
        assert!(is_external_web_url("https://www.ign.es/"));
        assert_eq!(without_verbatim_prefix(Path::new(r"\\?\D:\app")), PathBuf::from(r"D:\app"));
        assert_eq!(
            without_verbatim_prefix(Path::new(r"\\?\UNC\srv\share\app")),
            PathBuf::from(r"\\srv\share\app")
        );
        assert_eq!(without_verbatim_prefix(Path::new(r"D:\app")), PathBuf::from(r"D:\app"));
        assert!(!is_external_web_url("file:///C:/x"));
    }
}
