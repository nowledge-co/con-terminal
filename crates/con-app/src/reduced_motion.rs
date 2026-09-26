//! Keeps GPUI's reduced-motion flag synchronized with the operating system.
//!
//! `gpui-base` performs the initial platform read and follows the Linux XDG
//! Settings portal. This module adds the native live notifications that Base
//! cannot install on macOS and Windows. An absent/unsupported Linux portal is
//! deliberately treated as “unknown”, leaving GPUI's existing value intact.

use gpui::App;

pub fn init(cx: &mut App) {
    // gpui_component::init already reads the initial preference and follows
    // Linux's portal. Only the missing native subscriptions belong here.
    platform::follow(cx);
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::c_void;

    use gpui::{App, Global};
    use objc::{
        class,
        declare::ClassDecl,
        msg_send,
        runtime::{Class, Object, Sel},
        sel, sel_impl,
    };

    struct Observer(*mut Object);
    impl Global for Observer {}

    impl Drop for Observer {
        fn drop(&mut self) {
            unsafe {
                let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
                let center: *mut Object = msg_send![workspace, notificationCenter];
                let _: () = msg_send![center, removeObserver:self.0];
                let _: () = msg_send![self.0, release];
            }
        }
    }

    type Sender = futures::channel::mpsc::UnboundedSender<()>;

    extern "C" fn changed(this: &Object, _: Sel, _: *mut Object) {
        unsafe {
            let sender = *this.get_ivar::<*mut c_void>("sender") as *mut Sender;
            let _ = (&*sender).unbounded_send(());
        }
    }

    extern "C" fn dealloc(this: &mut Object, _: Sel) {
        unsafe {
            let sender = *this.get_ivar::<*mut c_void>("sender") as *mut Sender;
            drop(Box::from_raw(sender));
            let superclass = class!(NSObject);
            let _: () = msg_send![super(this, superclass), dealloc];
        }
    }

    fn observer_class() -> &'static Class {
        static CLASS: std::sync::OnceLock<&'static Class> = std::sync::OnceLock::new();
        CLASS.get_or_init(|| {
            let mut decl = ClassDecl::new("ConReducedMotionObserver", class!(NSObject)).unwrap();
            decl.add_ivar::<*mut c_void>("sender");
            unsafe {
                decl.add_method(
                    sel!(displayOptionsChanged:),
                    changed as extern "C" fn(&Object, Sel, *mut Object),
                );
                decl.add_method(sel!(dealloc), dealloc as extern "C" fn(&mut Object, Sel));
            }
            decl.register()
        })
    }

    fn read() -> bool {
        unsafe {
            let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
            msg_send![workspace, accessibilityDisplayShouldReduceMotion]
        }
    }

    pub(super) fn follow(cx: &mut App) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        cx.spawn(async move |cx| {
            use futures::StreamExt as _;
            while rx.next().await.is_some() {
                cx.update(|cx| cx.set_reduce_motion(read()));
            }
        })
        .detach();

        unsafe {
            let observer: *mut Object = msg_send![observer_class(), new];
            (*observer).set_ivar("sender", Box::into_raw(Box::new(tx)) as *mut c_void);
            let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
            let center: *mut Object = msg_send![workspace, notificationCenter];
            let name: *mut Object = msg_send![class!(NSString), stringWithUTF8String:b"NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification\0".as_ptr()];
            let _: () = msg_send![center, addObserver:observer selector:sel!(displayOptionsChanged:) name:name object:std::ptr::null::<Object>()];
            cx.set_global(Observer(observer));
        }
        // Close the race between the initial component read and subscription.
        cx.set_reduce_motion(read());
    }
}

#[cfg(any(
    target_os = "linux",
    not(any(target_os = "macos", target_os = "windows", target_os = "linux"))
))]
mod platform {
    use gpui::App;
    pub(super) fn follow(_: &mut App) {}
}

#[cfg(target_os = "windows")]
mod platform {
    use std::{ffi::c_void, thread::JoinHandle};

    use gpui::{App, Global};
    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::WindowsAndMessaging::{
            CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
            GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, MSG, PostMessageW, PostQuitMessage,
            RegisterClassW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
            SetWindowLongPtrW, SystemParametersInfoW, TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE,
            WM_DESTROY, WM_NCCREATE, WM_SETTINGCHANGE, WNDCLASSW, WS_OVERLAPPED,
        },
    };
    use windows::core::{BOOL, w};

    struct Watcher {
        hwnd: HWND,
        thread: Option<JoinHandle<()>>,
    }

    impl Global for Watcher {}

    impl Drop for Watcher {
        fn drop(&mut self) {
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    type Sender = futures::channel::mpsc::UnboundedSender<bool>;

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_NCCREATE {
            let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
        } else if message == WM_SETTINGCHANGE {
            let sender = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Sender };
            if !sender.is_null() {
                if let Some(reduce) = read() {
                    let _ = unsafe { &*sender }.unbounded_send(reduce);
                }
            }
        } else if message == WM_CLOSE {
            let _ = unsafe { DestroyWindow(hwnd) };
            return LRESULT(0);
        } else if message == WM_DESTROY {
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    pub(super) fn follow(cx: &mut App) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<bool>();
        cx.spawn(async move |cx| {
            use futures::StreamExt as _;
            while let Some(reduce) = rx.next().await {
                cx.update(|cx| cx.set_reduce_motion(reduce));
            }
        })
        .detach();

        let (hwnd_tx, hwnd_rx) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || unsafe {
            let class = w!("ConReducedMotionWatcher");
            let window_class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassW(&window_class) == 0 {
                let _ = hwnd_tx.send(None);
                return;
            }
            let sender = Box::new(tx);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                w!(""),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                Some((&*sender as *const Sender).cast::<c_void>()),
            )
            .ok();
            // HWND contains a raw pointer and is not Send. Only transfer its
            // numeric identity; the watcher thread remains its sole owner.
            let _ = hwnd_tx.send(hwnd.map(|hwnd| hwnd.0 as usize));
            let Some(_hwnd) = hwnd else {
                return;
            };
            if let Some(reduce) = read() {
                let _ = sender.unbounded_send(reduce);
            }
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let _ = DestroyWindow(_hwnd);
            drop(sender);
        });

        match hwnd_rx.recv() {
            Ok(Some(hwnd)) => cx.set_global(Watcher {
                hwnd: HWND(hwnd as *mut c_void),
                thread: Some(thread),
            }),
            _ => {
                let _ = thread.join();
                log::warn!("could not subscribe to Windows animation settings");
            }
        }
    }

    fn read() -> Option<bool> {
        let mut enabled = BOOL(0);
        unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                Some((&mut enabled as *mut BOOL).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        }
        .ok()?;
        Some(!enabled.as_bool())
    }
}
