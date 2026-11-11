//! A plugin's own editor, in a window of its own (Windows only for now).
//!
//! The window belongs to the UI thread, whose message loop (the one the
//! AIDAW window runs on) also serves it, as VST3 editors expect.

use std::sync::Arc;

use anyhow::Result;

use crate::rack::Slot;

/// An open plugin window.
pub struct Editor {
    /// The plugin it shows; kept alive while the window is open.
    pub slot: Arc<Slot>,
    #[cfg(windows)]
    win: win::Window,
}

impl Editor {
    /// Open the plugin's editor in a new window titled `title`.
    pub fn open(slot: Arc<Slot>, title: &str) -> Result<Editor> {
        #[cfg(windows)]
        {
            let win = win::Window::open(&slot, title)?;
            Ok(Editor { slot, win })
        }
        #[cfg(not(windows))]
        {
            let _ = title;
            anyhow::bail!(
                "{}'s own window can only be shown on Windows",
                slot.plugin.name
            )
        }
    }

    /// Whether the window is still open (the user may have closed it).
    pub fn is_open(&self) -> bool {
        #[cfg(windows)]
        {
            self.win.is_open()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// Bring the window to the front.
    pub fn focus(&self) {
        #[cfg(windows)]
        self.win.focus();
    }

    /// The size of the window's inside in pixels, while it is open.
    pub fn size(&self) -> Option<(i32, i32)> {
        #[cfg(windows)]
        {
            self.win.size()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

/// Whether plugin windows can be shown on this platform.
pub const SUPPORTED: bool = cfg!(windows);

#[cfg(windows)]
mod win {
    use std::cell::Cell;
    use std::ptr;
    use std::rc::Rc;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    use anyhow::{anyhow, bail, Result};
    use vst3::Steinberg::Vst::IEditControllerTrait;
    use vst3::Steinberg::*;
    use vst3::{Class, ComPtr, ComRef, ComWrapper};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use crate::rack::Slot;

    /// What the window procedure needs, kept behind GWLP_USERDATA.
    struct State {
        view: ComPtr<IPlugView>,
        /// Set while the window follows a size the plugin asked for.
        resizing: Cell<bool>,
        open: Rc<Cell<bool>>,
        attached: Cell<bool>,
    }

    impl State {
        fn detach(&self) {
            if self.attached.replace(false) {
                unsafe {
                    self.view.removed();
                    self.view.setFrame(ptr::null_mut());
                }
            }
        }
    }

    /// Lets the plugin resize its window.
    struct Frame {
        hwnd: Cell<HWND>,
        state: Cell<*const State>,
    }

    // SAFETY: only used on the UI thread that owns the window.
    unsafe impl Send for Frame {}
    unsafe impl Sync for Frame {}

    impl Class for Frame {
        type Interfaces = (IPlugFrame,);
    }

    impl IPlugFrameTrait for Frame {
        unsafe fn resizeView(&self, view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
            let hwnd = self.hwnd.get();
            if hwnd.is_null() || new_size.is_null() {
                return kInvalidArgument;
            }
            let r = unsafe { *new_size };
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            if w <= 0 || h <= 0 {
                return kInvalidArgument;
            }
            let state = self.state.get();
            unsafe {
                if !state.is_null() {
                    (*state).resizing.set(true);
                }
                set_client_size(hwnd, w, h);
                if !state.is_null() {
                    (*state).resizing.set(false);
                }
                match ComRef::from_raw(view) {
                    Some(v) => v.onSize(new_size),
                    None => kResultOk,
                }
            }
        }
    }

    unsafe fn set_client_size(hwnd: HWND, w: i32, h: i32) {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        unsafe {
            let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            AdjustWindowRectEx(&mut rect, style, 0, ex);
            SetWindowPos(
                hwnd,
                ptr::null_mut(),
                0,
                0,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const State;
        match msg {
            WM_SIZE if !state.is_null() => {
                let s = unsafe { &*state };
                if !s.resizing.get() && s.attached.get() && wp as u32 != SIZE_MINIMIZED {
                    let mut r = ViewRect {
                        left: 0,
                        top: 0,
                        right: (lp & 0xFFFF) as i32,
                        bottom: ((lp >> 16) & 0xFFFF) as i32,
                    };
                    if unsafe { s.view.canResize() } == kResultTrue {
                        unsafe {
                            s.view.checkSizeConstraint(&mut r);
                            s.view.onSize(&mut r);
                        }
                    }
                }
                0
            }
            WM_DPICHANGED if !state.is_null() => {
                let s = unsafe { &*state };
                let dpi = (wp & 0xFFFF) as f32;
                if let Some(scale) = s.view.cast::<IPlugViewContentScaleSupport>() {
                    unsafe { scale.setContentScaleFactor(dpi / 96.0) };
                }
                unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
            }
            WM_CLOSE => {
                if !state.is_null() {
                    let s = unsafe { &*state };
                    s.detach();
                    s.open.set(false);
                }
                unsafe { DestroyWindow(hwnd) };
                0
            }
            WM_DESTROY => {
                if !state.is_null() {
                    unsafe {
                        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                        let s = Box::from_raw(state as *mut State);
                        s.detach();
                        s.open.set(false);
                    }
                }
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn class_name() -> &'static [u16] {
        static NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
        NAME.get_or_init(|| {
            let name = wide("AIDAWPluginEditor");
            unsafe {
                let wc = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(wndproc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: GetModuleHandleW(ptr::null()),
                    hIcon: ptr::null_mut(),
                    hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
                    hbrBackground: GetStockObject(BLACK_BRUSH) as HBRUSH,
                    lpszMenuName: ptr::null(),
                    lpszClassName: name.as_ptr(),
                    hIconSm: ptr::null_mut(),
                };
                RegisterClassExW(&wc);
            }
            name
        })
    }

    pub struct Window {
        hwnd: HWND,
        open: Rc<Cell<bool>>,
        _frame: ComWrapper<Frame>,
    }

    impl Window {
        pub fn open(slot: &Arc<Slot>, title: &str) -> Result<Window> {
            let plugin = &slot.plugin;
            let controller = plugin
                .controller()
                .ok_or_else(|| anyhow!("{} has no window of its own", plugin.name))?;
            let view = unsafe { controller.createView(vst3::Steinberg::Vst::ViewType::kEditor) };
            let view = unsafe { ComPtr::from_raw(view) }
                .ok_or_else(|| anyhow!("{} has no window of its own", plugin.name))?;
            if unsafe { view.isPlatformTypeSupported(kPlatformTypeHWND) } != kResultTrue {
                bail!("{}'s window does not work on Windows", plugin.name);
            }
            let mut size = ViewRect {
                left: 0,
                top: 0,
                right: 640,
                bottom: 400,
            };
            unsafe { view.getSize(&mut size) };
            let resizable = unsafe { view.canResize() } == kResultTrue;
            let style = WS_OVERLAPPED
                | WS_CAPTION
                | WS_SYSMENU
                | WS_MINIMIZEBOX
                | WS_CLIPCHILDREN
                | if resizable {
                    WS_THICKFRAME | WS_MAXIMIZEBOX
                } else {
                    0
                };
            let title = wide(title);
            let owner = unsafe { GetActiveWindow() };
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class_name().as_ptr(),
                    title.as_ptr(),
                    style,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    640,
                    400,
                    owner,
                    ptr::null_mut(),
                    GetModuleHandleW(ptr::null()),
                    ptr::null(),
                )
            };
            if hwnd.is_null() {
                bail!("could not open a window for {}", plugin.name);
            }
            unsafe {
                set_client_size(
                    hwnd,
                    (size.right - size.left).max(64),
                    (size.bottom - size.top).max(32),
                )
            };

            let open = Rc::new(Cell::new(true));
            let state = Box::into_raw(Box::new(State {
                view: Clone::clone(&view),
                resizing: Cell::new(false),
                open: open.clone(),
                attached: Cell::new(false),
            }));
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize) };
            let frame = ComWrapper::new(Frame {
                hwnd: Cell::new(hwnd),
                state: Cell::new(state),
            });
            unsafe {
                if let Some(f) = frame.as_com_ref::<IPlugFrame>() {
                    view.setFrame(f.as_ptr());
                }
                let dpi = GetDpiForWindow(hwnd);
                if let Some(scale) = view.cast::<IPlugViewContentScaleSupport>() {
                    if dpi > 0 {
                        scale.setContentScaleFactor(dpi as f32 / 96.0);
                    }
                }
                if view.attached(hwnd, kPlatformTypeHWND) != kResultOk {
                    view.setFrame(ptr::null_mut());
                    DestroyWindow(hwnd);
                    bail!("{} could not show its window", plugin.name);
                }
                (*state).attached.set(true);
                // Its size may have changed once it could see the screen.
                let mut now = size;
                if view.getSize(&mut now) == kResultOk
                    && (now.right - now.left, now.bottom - now.top)
                        != (size.right - size.left, size.bottom - size.top)
                {
                    (*state).resizing.set(true);
                    set_client_size(hwnd, now.right - now.left, now.bottom - now.top);
                    (*state).resizing.set(false);
                }
                ShowWindow(hwnd, SW_SHOW);
                SetForegroundWindow(hwnd);
            }
            plugin.editor_open.store(true, Ordering::Release);
            Ok(Window {
                hwnd,
                open,
                _frame: frame,
            })
        }

        pub fn is_open(&self) -> bool {
            self.open.get() && unsafe { IsWindow(self.hwnd) } != 0
        }

        pub fn focus(&self) {
            if self.is_open() {
                unsafe {
                    ShowWindow(self.hwnd, SW_RESTORE);
                    SetForegroundWindow(self.hwnd);
                }
            }
        }

        pub fn size(&self) -> Option<(i32, i32)> {
            let mut r = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            (self.is_open() && unsafe { GetClientRect(self.hwnd, &mut r) } != 0)
                .then(|| (r.right - r.left, r.bottom - r.top))
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            if self.is_open() {
                // WM_DESTROY detaches the view and frees the state.
                unsafe { DestroyWindow(self.hwnd) };
            }
        }
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.slot
            .plugin
            .editor_open
            .store(false, std::sync::atomic::Ordering::Release);
    }
}
