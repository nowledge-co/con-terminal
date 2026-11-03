//! Main-thread, gesture-scoped AppKit observation. No global event tap or input
//! permission, and no callback re-enters the GPUI update which owns the drag.
use gpui::{Hsla, Pixels, Point, Window, point, px};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{ffi::CString, marker::PhantomData, ptr::NonNull, rc::Rc};

unsafe extern "C" {
    fn con_session_drag_begin(
        view: *mut std::ffi::c_void,
        title: *const std::ffi::c_char,
        br: f64,
        bg: f64,
        bb: f64,
        fr: f64,
        fg: f64,
        fb: f64,
    ) -> *mut std::ffi::c_void;
    fn con_session_drag_sample(
        capture: *mut std::ffi::c_void,
        x: *mut f64,
        y: *mut f64,
        outside: *mut bool,
    ) -> i32;
    fn con_session_drag_end(capture: *mut std::ffi::c_void);
}

pub(crate) struct SessionDragCapture {
    native: NonNull<std::ffi::c_void>,
    // AppKit objects and their Drop must remain on the foreground executor.
    _main_thread: PhantomData<Rc<()>>,
}

pub(crate) struct Sample {
    pub position: Point<Pixels>,
    pub outside: bool,
    pub released: bool,
    pub cancelled: bool,
}

impl SessionDragCapture {
    pub fn start(window: &Window, title: &str, background: Hsla, foreground: Hsla) -> Option<Self> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        let title = CString::new(title.replace('\0', "")).ok()?;
        let background = background.to_rgb();
        let foreground = foreground.to_rgb();
        let native = NonNull::new(unsafe {
            con_session_drag_begin(
                handle.ns_view.as_ptr(),
                title.as_ptr(),
                background.r.into(),
                background.g.into(),
                background.b.into(),
                foreground.r.into(),
                foreground.g.into(),
                foreground.b.into(),
            )
        })?;
        Some(Self {
            native,
            _main_thread: PhantomData,
        })
    }

    pub fn sample(&self) -> Sample {
        let (mut x, mut y, mut outside) = (0.0, 0.0, false);
        let status =
            unsafe { con_session_drag_sample(self.native.as_ptr(), &mut x, &mut y, &mut outside) };
        Sample {
            position: point(px(x as f32), px(y as f32)),
            outside,
            released: status == 1,
            cancelled: status == 2,
        }
    }
}

impl Drop for SessionDragCapture {
    fn drop(&mut self) {
        unsafe { con_session_drag_end(self.native.as_ptr()) };
    }
}
