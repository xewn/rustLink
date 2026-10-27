use crate::app::AppState;
use tokio_util::sync::CancellationToken;

pub fn open_browser(url: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(0x08000000)
            .spawn();
    }
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
}
#[cfg(not(windows))]
pub fn start(_: AppState, _: String, _: CancellationToken) {}

#[cfg(windows)]
pub fn start(state: AppState, url: String, shutdown: CancellationToken) {
    let handle = tokio::runtime::Handle::current();
    std::thread::spawn(move || windows::run(state, url, shutdown, handle));
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{cell::RefCell, ptr::null_mut};
    use windows_sys::Win32::{
        Foundation::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{Shell::*, WindowsAndMessaging::*},
    };
    struct Context {
        state: AppState,
        url: String,
        shutdown: CancellationToken,
        runtime: tokio::runtime::Handle,
    }
    thread_local! {static CONTEXT:RefCell<Option<Context>>=const{RefCell::new(None)};}
    const TRAY_MESSAGE: u32 = WM_APP + 1;
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    // All Win32 handles and the message callback remain on this dedicated thread.
    unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        unsafe {
            if msg == TRAY_MESSAGE {
                if l as u32 == WM_LBUTTONDBLCLK {
                    CONTEXT.with_borrow(|c| {
                        if let Some(c) = c {
                            open_browser(&c.url)
                        }
                    });
                }
                if l as u32 == WM_RBUTTONUP {
                    let menu = CreatePopupMenu();
                    for (id, label) in [
                        (1, "打开配置页面"),
                        (2, "启动采集"),
                        (3, "停止采集"),
                        (4, "退出"),
                    ] {
                        AppendMenuW(menu, MF_STRING, id, wide(label).as_ptr());
                    }
                    let mut point: POINT = std::mem::zeroed();
                    GetCursorPos(&mut point);
                    SetForegroundWindow(hwnd);
                    let command = TrackPopupMenu(
                        menu,
                        TPM_RETURNCMD | TPM_RIGHTBUTTON,
                        point.x,
                        point.y,
                        0,
                        hwnd,
                        null_mut(),
                    );
                    DestroyMenu(menu);
                    CONTEXT.with_borrow(|context| {
                        if let Some(c) = context {
                            match command {
                                1 => open_browser(&c.url),
                                2 => {
                                    let s = c.state.clone();
                                    c.runtime.spawn(async move {
                                        if let Ok(cfg) = serde_json::from_value(s.store.snapshot())
                                        {
                                            let _ = s.engine.start(cfg).await;
                                        }
                                    });
                                }
                                3 => {
                                    let s = c.state.clone();
                                    c.runtime.spawn(async move {
                                        s.engine.stop().await;
                                    });
                                }
                                4 => {
                                    c.shutdown.cancel();
                                    DestroyWindow(hwnd);
                                }
                                _ => {}
                            }
                        }
                    });
                }
                return 0;
            }
            if msg == WM_TIMER {
                let stopped =
                    CONTEXT.with_borrow(|c| c.as_ref().is_some_and(|c| c.shutdown.is_cancelled()));
                if stopped {
                    DestroyWindow(hwnd);
                }
                return 0;
            }
            if msg == WM_DESTROY {
                PostQuitMessage(0);
                return 0;
            }
            DefWindowProcW(hwnd, msg, w, l)
        }
    }
    pub fn run(
        state: AppState,
        url: String,
        shutdown: CancellationToken,
        runtime: tokio::runtime::Handle,
    ) {
        CONTEXT.with_borrow_mut(|c| {
            *c = Some(Context {
                state,
                url,
                shutdown,
                runtime,
            })
        });
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = wide("RustLinkTrayWindow");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            if RegisterClassW(&wc) == 0 {
                return;
            }
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                wide("RustLink").as_ptr(),
                0,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                null_mut(),
            );
            if hwnd.is_null() {
                return;
            }
            let mut icon: NOTIFYICONDATAW = std::mem::zeroed();
            icon.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            icon.hWnd = hwnd;
            icon.uID = 1;
            icon.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            icon.uCallbackMessage = TRAY_MESSAGE;
            icon.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
            let tip = wide("RustLink 工业数据采集网关");
            icon.szTip[..tip.len()].copy_from_slice(&tip);
            Shell_NotifyIconW(NIM_ADD, &icon);
            SetTimer(hwnd, 1, 500, None);
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            Shell_NotifyIconW(NIM_DELETE, &icon);
            UnregisterClassW(class.as_ptr(), instance);
        }
        CONTEXT.with_borrow_mut(|c| *c = None);
    }
}
