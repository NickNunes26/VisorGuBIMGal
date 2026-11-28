//! The Windows part of the shell: window, single instance, WebView2, dialogs, file streams.
//! Everything runs on the UI thread. Work that shows a modal dialog is queued and run from the
//! message loop (`WM_APP_RUN_JOBS`), never inside a WebView2 event handler, and never while the
//! shell state is borrowed: a dialog pumps messages and the window procedure needs that state.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use serde_json::{json, Value};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    CoTaskMemPWSTR, CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, WebMessageReceivedEventHandler, WebResourceRequestedEventHandler,
};
use windows::core::{w, Interface, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, UpdateWindow, HBRUSH};
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    GetDpiForSystem, GetDpiForWindow, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::logic::{self, FileRegistry, FileToken, PendingFiles};

const CLASS_NAME: PCWSTR = w!("VisorGuBIMGalWindow");
const WINDOW_TITLE: &str = "VisorGuBIMGal — Visor BIM/GIS";
const MUTEX_NAME: PCWSTR = w!("Local\\VisorGuBIMGal.Instance");
const WM_APP_RUN_JOBS: u32 = WM_APP + 1;
/// Background of the window and of the WebView before the page paints (#0f172a).
const BG: (u8, u8, u8) = (0x0f, 0x17, 0x2a);

struct Shell {
    env: ICoreWebView2Environment,
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    registry: FileRegistry,
    pending: PendingFiles,
    dev_url: Option<String>,
    /// Origin allowed to read `gubim-file.localhost` (the app, or the dev server).
    allow_origin: String,
}

thread_local! {
    static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
    static JOBS: RefCell<VecDeque<Box<dyn FnOnce()>>> = RefCell::new(VecDeque::new());
    static MAIN_HWND: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
}

/// Runs `f` on the shell state. `None` before it exists, or if it is already borrowed further
/// up the stack (a re-entrant call), which the code below is arranged never to need.
fn with_shell<R>(f: impl FnOnce(&mut Shell) -> R) -> Option<R> {
    SHELL.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

fn post_job(job: impl FnOnce() + 'static) {
    JOBS.with(|j| j.borrow_mut().push_back(Box::new(job)));
    let hwnd = MAIN_HWND.with(Cell::get);
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_APP_RUN_JOBS, WPARAM(0), LPARAM(0));
    }
}

fn run_jobs() {
    while let Some(job) = JOBS.with(|j| j.borrow_mut().pop_front()) {
        job();
    }
}

pub fn fatal(message: &str) {
    unsafe {
        MessageBoxW(None, &HSTRING::from(message), w!("VisorGuBIMGal"), MB_ICONERROR | MB_OK);
    }
}

pub fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| format!("COM: {e}"))?;
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let files = logic::collect_ifc_args(&args);
    // The handle stays open for the life of the process: it is what marks the first instance.
    let _mutex = unsafe { CreateMutexW(None, true, MUTEX_NAME) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        forward_to_running_instance(&files);
        return Ok(());
    }

    let dev_url = logic::flag_value(&args, "dev-url");
    let hwnd = create_window()?;
    MAIN_HWND.with(|h| h.set(hwnd));

    let env = create_environment()?;
    let controller = create_controller(&env, hwnd)?;
    let webview = unsafe { controller.CoreWebView2() }.map_err(|e| e.to_string())?;
    configure(&controller, &webview).map_err(|e| e.to_string())?;

    let allow_origin = match &dev_url {
        Some(dev) => origin_of(dev),
        None => logic::app_origin(),
    };
    let mut shell = Shell {
        env,
        controller,
        webview: webview.clone(),
        registry: FileRegistry::default(),
        pending: PendingFiles::default(),
        dev_url: dev_url.clone(),
        allow_origin,
    };
    for path in &files {
        if let Some(tok) = shell.registry.register(path) {
            shell.pending.offer(tok);
        }
    }
    SHELL.with(|s| *s.borrow_mut() = Some(shell));

    add_handlers(&webview).map_err(|e| e.to_string())?;
    resize_webview(hwnd);

    let start_url = match &dev_url {
        Some(dev) => dev.clone(),
        None => {
            let dir = app_dir(&args)?;
            let wv3: ICoreWebView2_3 = webview.cast().map_err(|e| e.to_string())?;
            unsafe {
                wv3.SetVirtualHostNameToFolderMapping(
                    &HSTRING::from(logic::APP_HOST),
                    &HSTRING::from(dir.as_os_str()),
                    COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS,
                )
            }
            .map_err(|e| e.to_string())?;
            format!("{}/index.html", logic::app_origin())
        }
    };
    unsafe {
        webview.Navigate(&HSTRING::from(start_url)).map_err(|e| e.to_string())?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
    }

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

/// The build folder to serve: `--app-dir`, else `app` next to the executable.
fn app_dir(args: &[String]) -> Result<PathBuf, String> {
    let dir = match logic::flag_value(args, "app-dir") {
        Some(d) => PathBuf::from(d),
        None => std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .map(|p| p.join("app"))
            .unwrap_or_default(),
    };
    // canonicalize gives a verbatim `\\?\D:\...` path; WebView2's folder mapping serves
    // index.html from it but fails every sub-path (ERR_INVALID_URL), so use the plain form.
    let dir = std::fs::canonicalize(&dir)
        .map(|p| logic::without_verbatim_prefix(&p))
        .unwrap_or(dir);
    if !dir.join("index.html").is_file() {
        return Err(format!("No se encuentra la aplicación en {}", dir.display()));
    }
    Ok(dir)
}

fn origin_of(url: &str) -> String {
    let after_scheme = url.find("://").map(|i| i + 3).unwrap_or(0);
    let end = url[after_scheme..].find('/').map(|i| after_scheme + i).unwrap_or(url.len());
    url[..end].to_string()
}

// ------------------------------------------------------------------------------------------
// Single instance

fn forward_to_running_instance(files: &[PathBuf]) {
    let absolute: Vec<PathBuf> = files
        .iter()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();
    let payload = logic::encode_open_request(&absolute);
    // The first instance may still be creating its window: wait up to 5 s for it.
    for _ in 0..50 {
        if let Ok(target) = unsafe { FindWindowW(CLASS_NAME, PCWSTR::null()) } {
            unsafe {
                let mut pid = 0u32;
                GetWindowThreadProcessId(target, Some(&mut pid));
                let _ = AllowSetForegroundWindow(pid);
                let cds = COPYDATASTRUCT {
                    dwData: logic::COPYDATA_OPEN_FILES,
                    cbData: (payload.len() * 2) as u32,
                    lpData: payload.as_ptr() as *mut c_void,
                };
                let mut result = 0usize;
                let _ = SendMessageTimeoutW(
                    target,
                    WM_COPYDATA,
                    WPARAM(0),
                    LPARAM(&cds as *const _ as isize),
                    SMTO_ABORTIFHUNG,
                    10_000,
                    Some(&mut result),
                );
            }
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Files from a second instance: queue them until the page asks, or send them as events.
fn receive_files(paths: Vec<PathBuf>) {
    let events = with_shell(|s| {
        let mut out = Vec::new();
        for p in paths.iter().filter(|p| logic::is_ifc_path(p)) {
            if let Some(tok) = s.registry.register(p) {
                if let Some(ready) = s.pending.offer(tok) {
                    out.push(logic::open_file_event(&ready));
                }
            }
        }
        out
    })
    .unwrap_or_default();
    for ev in events {
        post_to_page(&ev);
    }
}

fn bring_to_front(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

// ------------------------------------------------------------------------------------------
// Window

fn create_window() -> Result<HWND, String> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).map_err(|e| e.to_string())?.into();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hIcon: LoadIconW(Some(instance), PCWSTR(1 as _)).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(CreateSolidBrush(COLORREF(
                (BG.2 as u32) << 16 | (BG.1 as u32) << 8 | BG.0 as u32,
            ))
            .0),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err("RegisterClassExW".into());
        }
        let scale = |v: i32| v * GetDpiForSystem() as i32 / 96;
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS_NAME,
            &HSTRING::from(WINDOW_TITLE),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            scale(1440),
            scale(900),
            None,
            None,
            Some(instance),
            None,
        )
        .map_err(|e| e.to_string())
    }
}

fn resize_webview(hwnd: HWND) {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    with_shell(|s| unsafe {
        let _ = s.controller.SetBounds(rect);
    });
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            let minimized = wparam.0 as u32 == SIZE_MINIMIZED;
            with_shell(|s| unsafe {
                let _ = s.controller.SetIsVisible(!minimized);
            });
            if !minimized {
                resize_webview(hwnd);
            }
            LRESULT(0)
        }
        WM_MOVE | WM_MOVING => {
            with_shell(|s| unsafe {
                let _ = s.controller.NotifyParentWindowPositionChanged();
            });
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_ACTIVATE | WM_SETFOCUS => {
            with_shell(|s| unsafe {
                let _ = s.controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
            });
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_GETMINMAXINFO => {
            let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
            let info = lparam.0 as *mut MINMAXINFO;
            if !info.is_null() {
                unsafe {
                    (*info).ptMinTrackSize = POINT { x: 1024 * dpi / 96, y: 640 * dpi / 96 };
                }
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let r = lparam.0 as *const RECT;
            if !r.is_null() {
                unsafe {
                    let r = *r;
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            LRESULT(0)
        }
        WM_COPYDATA => {
            let cds = lparam.0 as *const COPYDATASTRUCT;
            if cds.is_null() {
                return LRESULT(0);
            }
            let cds = unsafe { &*cds };
            if cds.dwData != logic::COPYDATA_OPEN_FILES {
                return LRESULT(0);
            }
            let data = if cds.lpData.is_null() || cds.cbData == 0 {
                Vec::new()
            } else {
                unsafe {
                    std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2)
                        .to_vec()
                }
            };
            bring_to_front(hwnd);
            // Answer the sender at once; registering and messaging the page can wait.
            post_job(move || receive_files(logic::decode_open_request(&data)));
            LRESULT(1)
        }
        WM_APP_RUN_JOBS => {
            run_jobs();
            LRESULT(0)
        }
        WM_DESTROY => {
            SHELL.with(|s| {
                if let Ok(mut s) = s.try_borrow_mut() {
                    if let Some(shell) = s.take() {
                        unsafe {
                            let _ = shell.controller.Close();
                        }
                    }
                }
            });
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

// ------------------------------------------------------------------------------------------
// WebView2

fn create_environment() -> Result<ICoreWebView2Environment, String> {
    // GUBIM_DESKTOP_DATA_DIR: a throwaway profile for the browser checks.
    let data_dir = match std::env::var_os("GUBIM_DESKTOP_DATA_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("VisorGuBIMGal")
            .join("WebView2"),
    };
    let _ = std::fs::create_dir_all(&data_dir);
    let data_dir = HSTRING::from(data_dir.as_os_str());
    let options: ICoreWebView2EnvironmentOptions = CoreWebView2EnvironmentOptions::default().into();
    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), &data_dir, &options, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |code, env| {
            let _ = tx.send(code.and_then(|_| env.ok_or_else(|| E_POINTER.into())));
            Ok(())
        }),
    )
    .map_err(|_| missing_runtime_message())?;
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|_| missing_runtime_message())
}

fn missing_runtime_message() -> String {
    "No se encontró Microsoft Edge WebView2 Runtime. Instálalo desde \
     https://developer.microsoft.com/microsoft-edge/webview2/ y vuelve a abrir la aplicación."
        .into()
}

fn create_controller(env: &ICoreWebView2Environment, hwnd: HWND) -> Result<ICoreWebView2Controller, String> {
    let (tx, rx) = mpsc::channel();
    let env = env.clone();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            env.CreateCoreWebView2Controller(hwnd, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |code, controller| {
            let _ = tx.send(code.and_then(|_| controller.ok_or_else(|| E_POINTER.into())));
            Ok(())
        }),
    )
    .map_err(|e| format!("WebView2: {e}"))?;
    rx.recv().map_err(|e| e.to_string())?.map_err(|e| format!("WebView2: {e}"))
}

fn configure(controller: &ICoreWebView2Controller, webview: &ICoreWebView2) -> windows::core::Result<()> {
    unsafe {
        if let Ok(c2) = controller.cast::<ICoreWebView2Controller2>() {
            c2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR { A: 255, R: BG.0, G: BG.1, B: BG.2 })?;
        }
        let settings = webview.Settings()?;
        settings.SetIsStatusBarEnabled(false)?;
        settings.SetIsZoomControlEnabled(false)?;
        if !cfg!(debug_assertions) {
            settings.SetAreDevToolsEnabled(false)?;
            settings.SetAreDefaultContextMenusEnabled(false)?;
        }
        controller.SetIsVisible(true)?;
    }
    Ok(())
}

/// Reads a string a WebView2 getter returns through a `PWSTR` out-parameter.
fn read_pwstr(f: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> Option<String> {
    let mut p = PWSTR::null();
    f(&mut p).ok()?;
    if p.is_null() {
        return None;
    }
    Some(CoTaskMemPWSTR::from(p).to_string())
}

fn post_to_page(json: &str) {
    let webview = with_shell(|s| s.webview.clone());
    if let Some(wv) = webview {
        unsafe {
            let _ = wv.PostWebMessageAsJson(&HSTRING::from(json));
        }
    }
}

fn open_external(url: &str) {
    if !logic::is_external_web_url(url) {
        return;
    }
    // Debug builds only: the browser checks must not open the user's browser, so they name a
    // file the URL is appended to instead.
    #[cfg(debug_assertions)]
    if let Some(log) = std::env::var_os("GUBIM_DESKTOP_TEST_EXTERNAL_LOG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
            let _ = writeln!(f, "{url}");
        }
        return;
    }
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL);
    }
}

fn add_handlers(webview: &ICoreWebView2) -> windows::core::Result<()> {
    let mut token = 0i64;
    unsafe {
        // Only the app may load in the window; links elsewhere open in the user's browser.
        webview.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(|_, args| {
                let Some(args) = args else { return Ok(()) };
                let uri = read_pwstr(|p| args.Uri(p)).unwrap_or_default();
                let dev = with_shell(|s| s.dev_url.clone()).flatten();
                if logic::is_app_url(&uri, dev.as_deref()) {
                    with_shell(|s| s.pending.page_reloading());
                } else {
                    args.SetCancel(true)?;
                    open_external(&uri);
                }
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(|_, args| {
                let Some(args) = args else { return Ok(()) };
                let uri = read_pwstr(|p| args.Uri(p)).unwrap_or_default();
                args.SetHandled(true)?;
                open_external(&uri);
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(|_, args| {
                let Some(args) = args else { return Ok(()) };
                let source = read_pwstr(|p| args.Source(p)).unwrap_or_default();
                let dev = with_shell(|s| s.dev_url.clone()).flatten();
                if !logic::is_app_url(&source, dev.as_deref()) {
                    return Ok(());
                }
                if let Some(req) = read_pwstr(|p| args.WebMessageAsJson(p))
                    .and_then(|j| logic::parse_request(&j))
                {
                    handle_request(req);
                }
                Ok(())
            })),
            &mut token,
        )?;
        webview.AddWebResourceRequestedFilter(
            &HSTRING::from(format!("https://{}/*", logic::FILE_HOST)),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        )?;
        // Every page load too: cancelling in NavigationStarting does not stop WebView2 from
        // sending the request, so a load of anything but the app is answered here, never sent.
        webview.AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT)?;
        webview.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    handle_resource(args);
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

fn handle_request(req: logic::HostRequest) {
    let id = req.id;
    match req.cmd.as_str() {
        "takePendingFiles" => {
            let files = with_shell(|s| s.pending.take_all()).unwrap_or_default();
            post_to_page(&logic::reply_ok(id, json!(files)));
        }
        "registerIfcPath" => {
            let path = req.args.get("path").and_then(Value::as_str).map(PathBuf::from);
            let tok = path
                .filter(|p| logic::is_ifc_path(p))
                .and_then(|p| with_shell(|s| s.registry.register(&p)).flatten());
            post_to_page(&logic::reply_ok(id, json!(tok)));
        }
        "pickIfcFiles" => post_job(move || {
            let hwnd = MAIN_HWND.with(Cell::get);
            let picked = pick_ifc_files(hwnd);
            let tokens: Vec<FileToken> = with_shell(|s| {
                picked.iter().filter_map(|p| s.registry.register(p)).collect()
            })
            .unwrap_or_default();
            post_to_page(&logic::reply_ok(id, json!(tokens)));
        }),
        other => post_to_page(&logic::reply_err(id, &format!("Orden desconocida: {other}"))),
    }
}

// ------------------------------------------------------------------------------------------
// gubim-file.localhost: reading local IFC files and saving exports

fn cors_headers(origin: &str) -> String {
    format!(
        "Access-Control-Allow-Origin: {origin}\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type\r\nTiming-Allow-Origin: {origin}\r\n\
         Cache-Control: no-store"
    )
}

fn respond(args: &ICoreWebView2WebResourceRequestedEventArgs, status: i32, reason: &str, body: Option<IStream>, extra: &str) {
    let Some((env, origin)) = with_shell(|s| (s.env.clone(), s.allow_origin.clone())) else {
        return;
    };
    let mut headers = cors_headers(&origin);
    if !extra.is_empty() {
        headers.push_str("\r\n");
        headers.push_str(extra);
    }
    unsafe {
        if let Ok(resp) = env.CreateWebResourceResponse(
            body.as_ref(),
            status,
            &HSTRING::from(reason),
            &HSTRING::from(headers),
        ) {
            let _ = args.SetResponse(&resp);
        }
    }
}

fn text_stream(text: &str) -> Option<IStream> {
    unsafe { SHCreateMemStream(Some(text.as_bytes())) }
}

fn handle_resource(args: ICoreWebView2WebResourceRequestedEventArgs) {
    let Ok(request) = (unsafe { args.Request() }) else { return };
    let uri = read_pwstr(|p| unsafe { request.Uri(p) }).unwrap_or_default();
    let method = read_pwstr(|p| unsafe { request.Method(p) }).unwrap_or_default();

    if !uri.starts_with(&format!("https://{}/", logic::FILE_HOST)) {
        // A page load (the DOCUMENT filter): the app's own pass untouched, the rest never leave.
        let dev = with_shell(|s| s.dev_url.clone()).flatten();
        if !logic::is_app_url(&uri, dev.as_deref()) {
            respond(&args, 403, "Forbidden", text_stream(""), "Content-Type: text/plain");
        }
        return;
    }
    if method.eq_ignore_ascii_case("OPTIONS") {
        respond(&args, 204, "No Content", None, "");
        return;
    }
    if let Some(query) = logic::save_query_from_url(&uri) {
        if !method.eq_ignore_ascii_case("POST") {
            respond(&args, 405, "Method Not Allowed", None, "");
            return;
        }
        let save = logic::parse_save_query(query);
        let body = unsafe { request.Content() }.ok().map(|s| read_stream(&s)).unwrap_or_default();
        // The dialog is modal: answer from the message loop, holding the request open meanwhile.
        let Ok(deferral) = (unsafe { args.GetDeferral() }) else { return };
        post_job(move || {
            let hwnd = MAIN_HWND.with(Cell::get);
            let (status, reason, text) = match save_file(hwnd, &save, &body) {
                Ok(true) => (200, "OK", "1".to_string()),
                Ok(false) => (200, "OK", "0".to_string()),
                Err(e) => (500, "Internal Server Error", e),
            };
            respond(&args, status, reason, text_stream(&text), "Content-Type: text/plain; charset=utf-8");
            unsafe {
                let _ = deferral.Complete();
            }
        });
        return;
    }
    let Some(token) = logic::file_token_from_url(&uri) else {
        respond(&args, 404, "Not Found", text_stream("Not found"), "Content-Type: text/plain");
        return;
    };
    let path = with_shell(|s| s.registry.resolve(&token).map(Path::to_path_buf)).flatten();
    let Some(path) = path else {
        respond(&args, 404, "Not Found", text_stream("Not found"), "Content-Type: text/plain");
        return;
    };
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    // One bounded range per request (logic::MAX_RANGE): WebView2 copies a whole response several
    // times inside this process, so the page reads the file range by range (E4).
    let (start, end) = match logic::parse_range(&uri, size) {
        Ok(r) => r,
        Err(status) => {
            respond(&args, status as i32, "Bad Range", text_stream("Bad range"), "Content-Type: text/plain");
            return;
        }
    };
    match read_range(&path, start, end) {
        Ok(bytes) => respond(
            &args,
            200,
            "OK",
            unsafe { SHCreateMemStream(Some(&bytes)) },
            &format!("Content-Type: application/octet-stream
Content-Length: {}", bytes.len()),
        ),
        Err(_) => respond(&args, 500, "Internal Server Error", text_stream("Read error"), "Content-Type: text/plain"),
    }
}

fn read_range(path: &Path, start: u64, end: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0u8; (end - start) as usize];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn read_stream(stream: &IStream) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let mut read = 0u32;
        let hr = unsafe { stream.Read(buf.as_mut_ptr() as *mut c_void, buf.len() as u32, Some(&mut read)) };
        if hr.is_err() || read == 0 {
            break;
        }
        out.extend_from_slice(&buf[..read as usize]);
    }
    out
}

// ------------------------------------------------------------------------------------------
// Native dialogs

fn shell_item_path(item: &IShellItem) -> Option<PathBuf> {
    unsafe {
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s.map(PathBuf::from)
    }
}

fn pick_ifc_files(owner: HWND) -> Vec<PathBuf> {
    unsafe {
        let Ok(dialog) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
        else {
            return Vec::new();
        };
        let name = HSTRING::from("Modelos IFC (*.ifc)");
        let spec = HSTRING::from("*.ifc");
        let filters = [COMDLG_FILTERSPEC { pszName: PCWSTR(name.as_ptr()), pszSpec: PCWSTR(spec.as_ptr()) }];
        let _ = dialog.SetFileTypes(&filters);
        let _ = dialog.SetTitle(&HSTRING::from("Abrir modelos IFC"));
        let opts = dialog.GetOptions().unwrap_or_default();
        let _ = dialog.SetOptions(opts | FOS_ALLOWMULTISELECT | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM);
        if dialog.Show(Some(owner)).is_err() {
            return Vec::new();
        }
        let Ok(items) = dialog.GetResults() else { return Vec::new() };
        let count = items.GetCount().unwrap_or(0);
        (0..count)
            .filter_map(|i| items.GetItemAt(i).ok())
            .filter_map(|item| shell_item_path(&item))
            .collect()
    }
}

/// Shows the save dialog and writes the file. `Ok(false)` when the user cancels.
fn save_file(owner: HWND, req: &logic::SaveRequest, bytes: &[u8]) -> Result<bool, String> {
    let target = save_target(owner, req);
    let Some(target) = target else { return Ok(false) };
    std::fs::write(&target, bytes).map_err(|e| format!("No se pudo escribir {}: {e}", target.display()))?;
    Ok(true)
}

fn save_target(owner: HWND, req: &logic::SaveRequest) -> Option<PathBuf> {
    // Debug builds only: the browser checks cannot click a native dialog, so they name a folder.
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("GUBIM_DESKTOP_TEST_SAVE_DIR") {
        let name = if req.default_name.is_empty() { "salida".to_string() } else { req.default_name.clone() };
        return Some(PathBuf::from(dir).join(name));
    }
    unsafe {
        let dialog = CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let label = HSTRING::from(if req.filter_name.is_empty() { "Archivo" } else { &req.filter_name });
        let spec = HSTRING::from(
            req.extensions.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(";"),
        );
        if !req.extensions.is_empty() {
            let filters = [COMDLG_FILTERSPEC { pszName: PCWSTR(label.as_ptr()), pszSpec: PCWSTR(spec.as_ptr()) }];
            let _ = dialog.SetFileTypes(&filters);
            let _ = dialog.SetDefaultExtension(&HSTRING::from(req.extensions[0].as_str()));
        }
        if !req.default_name.is_empty() {
            let _ = dialog.SetFileName(&HSTRING::from(req.default_name.as_str()));
        }
        let opts = dialog.GetOptions().unwrap_or_default();
        let _ = dialog.SetOptions(opts | FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM);
        dialog.Show(Some(owner)).ok()?;
        let item = dialog.GetResult().ok()?;
        shell_item_path(&item)
    }
}
