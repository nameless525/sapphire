//! The OS window and the native handles a renderer draws into.

use std::ffi::{c_int, c_ulong, c_void};
use std::num::NonZeroIsize;
use std::ptr::{self, NonNull};
use std::rc::Rc;

use raw_window_handle::{
    AndroidDisplayHandle, AndroidNdkWindowHandle, AppKitDisplayHandle, AppKitWindowHandle, DisplayHandle,
    HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, UiKitDisplayHandle,
    UiKitWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, Win32WindowHandle, WindowHandle,
    WindowsDisplayHandle, XlibDisplayHandle, XlibWindowHandle,
};
use sdl3_sys::init::{SDL_INIT_VIDEO, SDL_WasInit};
use sdl3_sys::metal::{SDL_Metal_CreateView, SDL_Metal_DestroyView, SDL_Metal_GetLayer};
use sdl3_sys::properties::{SDL_GetNumberProperty, SDL_GetPointerProperty, SDL_PropertiesID};
use sdl3_sys::rect::SDL_Rect;
use sdl3_sys::video::{
    SDL_CreateWindow, SDL_DestroyWindow, SDL_GetWindowDisplayScale, SDL_GetWindowID,
    SDL_GetWindowPixelDensity, SDL_GetWindowProperties, SDL_GetWindowSafeArea, SDL_GetWindowSize,
    SDL_GetWindowSizeInPixels, SDL_PROP_WINDOW_WAYLAND_DISPLAY_POINTER,
    SDL_PROP_WINDOW_WAYLAND_SURFACE_POINTER, SDL_PROP_WINDOW_WIN32_HWND_POINTER,
    SDL_PROP_WINDOW_WIN32_INSTANCE_POINTER, SDL_PROP_WINDOW_X11_DISPLAY_POINTER,
    SDL_PROP_WINDOW_X11_SCREEN_NUMBER, SDL_PROP_WINDOW_X11_WINDOW_NUMBER, SDL_WINDOW_FULLSCREEN,
    SDL_WINDOW_HIGH_PIXEL_DENSITY, SDL_WINDOW_METAL, SDL_WINDOW_RESIZABLE, SDL_Window,
};

use crate::{PlatformError, sdl};

/// Identifies a window in [`Event::Window`](crate::Event::Window) and input events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(pub(crate) u32);

/// A size in pixels or window coordinates, depending on where it comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// An axis-aligned rectangle in window coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// How to create the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowDesc {
    pub title: String,
    /// Initial size in window coordinates (the units of [`Window::size`]). Ignored on
    /// mobile.
    pub size: Size,
    /// Whether the user can resize the window. Ignored on mobile.
    pub resizable: bool,
    /// Borderless fullscreen on the window's display. Always on for mobile.
    pub fullscreen: bool,
}

impl Default for WindowDesc {
    fn default() -> Self {
        Self {
            title: String::from("Sapphire"),
            size: Size { width: 1280, height: 720 },
            resizable: true,
            fullscreen: false,
        }
    }
}

/// An OS window (M0 uses exactly one).
///
/// Created by [`Platform::create_window`](crate::Platform::create_window). Like
/// [`Platform`](crate::Platform) it stays on the main thread. The renderer draws into
/// it through a [`SurfaceHandle`] from [`surface_handle`](Self::surface_handle). The
/// native window is destroyed when the `Window` and every `SurfaceHandle` taken from it
/// have been dropped.
#[derive(Debug)]
pub struct Window {
    inner: Rc<WindowInner>,
}

/// The SDL window and the objects created with it, shared by the [`Window`] and its
/// [`SurfaceHandle`]s so the native window outlives every GPU surface built on it.
#[derive(Debug)]
struct WindowInner {
    raw: NonNull<SDL_Window>,
    id: WindowId,
    /// The SDL Metal view (an `NSView` / `UIView`) on Apple targets.
    metal_view: Option<NonNull<c_void>>,
    metal_layer: Option<NonNull<c_void>>,
}

impl Window {
    /// Creates the window. Only called through `Platform`, which guarantees SDL video
    /// is initialized and that we are on the main thread.
    pub(crate) fn create(desc: &WindowDesc) -> Result<Self, PlatformError> {
        let mobile = cfg!(any(target_os = "ios", target_os = "android"));
        let apple = cfg!(target_vendor = "apple");

        let mut flags = SDL_WINDOW_HIGH_PIXEL_DENSITY;
        if desc.resizable {
            flags |= SDL_WINDOW_RESIZABLE;
        }
        if desc.fullscreen || mobile {
            flags |= SDL_WINDOW_FULLSCREEN;
        }
        if apple {
            flags |= SDL_WINDOW_METAL;
        }
        let title = sdl::c_string_lossy(&desc.title);
        let width = c_int::try_from(desc.size.width).unwrap_or(c_int::MAX);
        let height = c_int::try_from(desc.size.height).unwrap_or(c_int::MAX);

        // SAFETY: SDL video is initialized and this is the main thread (see above); the
        // title is a valid C string that SDL copies.
        let raw = unsafe { SDL_CreateWindow(title.as_ptr(), width, height, flags) };
        let raw = NonNull::new(raw).ok_or_else(|| sdl::error("SDL_CreateWindow"))?;
        // SAFETY: `raw` is the live window just created.
        let id = WindowId(unsafe { SDL_GetWindowID(raw.as_ptr()) }.0);
        // From here on, `Drop` cleans up if a later step fails.
        let mut inner = WindowInner { raw, id, metal_view: None, metal_layer: None };

        if apple {
            // SAFETY: the window was created with SDL_WINDOW_METAL.
            let view = unsafe { SDL_Metal_CreateView(raw.as_ptr()) };
            let view = NonNull::new(view).ok_or_else(|| sdl::error("SDL_Metal_CreateView"))?;
            inner.metal_view = Some(view);
            // SAFETY: `view` is the live Metal view created above.
            let layer = unsafe { SDL_Metal_GetLayer(view.as_ptr()) };
            inner.metal_layer = Some(NonNull::new(layer).ok_or_else(|| sdl::error("SDL_Metal_GetLayer"))?);
        }
        Ok(Self { inner: Rc::new(inner) })
    }

    fn raw(&self) -> *mut SDL_Window {
        self.inner.raw.as_ptr()
    }

    pub fn id(&self) -> WindowId {
        self.inner.id
    }

    /// The size in window coordinates (the space mouse positions and
    /// [`safe_area`](Self::safe_area) use).
    pub fn size(&self) -> Size {
        let (mut w, mut h) = (0, 0);
        // SAFETY: the window is live and the out-pointers are valid; on failure SDL
        // leaves them at zero.
        unsafe { SDL_GetWindowSize(self.raw(), &mut w, &mut h) };
        to_size(w, h)
    }

    /// The drawable size in pixels: the size to configure the swapchain with.
    pub fn pixel_size(&self) -> Size {
        let (mut w, mut h) = (0, 0);
        // SAFETY: as in `size`.
        unsafe { SDL_GetWindowSizeInPixels(self.raw(), &mut w, &mut h) };
        to_size(w, h)
    }

    /// Pixels per window coordinate (2.0 on a Retina display).
    pub fn pixel_density(&self) -> f32 {
        // SAFETY: the window is live.
        unsafe { SDL_GetWindowPixelDensity(self.raw()) }
    }

    /// The scale UI should be drawn at, combining pixel density with the user's display
    /// scaling setting (for example 1.5 on a 150% Windows display).
    pub fn display_scale(&self) -> f32 {
        // SAFETY: the window is live.
        unsafe { SDL_GetWindowDisplayScale(self.raw()) }
    }

    /// The part of the window, in window coordinates, not covered by notches, rounded
    /// corners or system bars. Equals the whole window on desktop. Re-query after
    /// [`WindowEvent::SafeAreaChanged`](crate::WindowEvent::SafeAreaChanged).
    pub fn safe_area(&self) -> Rect {
        let mut rect = SDL_Rect::default();
        // SAFETY: the window is live and `rect` is a valid out-pointer.
        let ok = unsafe { SDL_GetWindowSafeArea(self.raw(), &mut rect) };
        if !ok {
            let Size { width, height } = self.size();
            return Rect { x: 0, y: 0, width, height };
        }
        let Size { width, height } = to_size(rect.w, rect.h);
        Rect { x: rect.x, y: rect.y, width, height }
    }

    /// Takes hold of the native objects a GPU surface for this window is created from.
    /// See [`SurfaceHandle`] for how long to keep it.
    ///
    /// # Errors
    ///
    /// - [`HandleError::Unavailable`] on Android while the app is in the background,
    ///   when the window has no `ANativeWindow`. Try again after
    ///   [`LifecycleEvent::DidEnterForeground`](crate::LifecycleEvent::DidEnterForeground).
    /// - [`HandleError::NotSupported`] under SDL video drivers that have no
    ///   raw-window-handle equivalent (offscreen, KMSDRM).
    pub fn surface_handle(&self) -> Result<SurfaceHandle, HandleError> {
        // SAFETY: the window is live.
        let props = unsafe { SDL_GetWindowProperties(self.raw()) };
        let android_window = AndroidWindowRef::acquire(props);
        let native =
            NativeHandles { android_window: android_window.get(), ..self.inner.native_handles(props) };
        // On error `android_window` drops here, releasing the reference just taken.
        let window = native.window_handle(WindowSystem::CURRENT)?;
        let display = native.display_handle(WindowSystem::CURRENT)?;
        Ok(SurfaceHandle {
            window,
            display,
            metal_layer: self.inner.metal_layer,
            _android_window: android_window,
            _owner: Rc::clone(&self.inner),
        })
    }
}

impl WindowInner {
    /// Reads the native objects SDL exposes for this window, except Android's native
    /// window, which [`AndroidWindowRef`] reads under the property lock.
    fn native_handles(&self, props: SDL_PropertiesID) -> NativeHandles {
        let pointer = |name| {
            // SAFETY: `name` is one of SDL's static property-name constants; reading an
            // absent property returns the null default. SDL locks properties internally.
            NonNull::new(unsafe { SDL_GetPointerProperty(props, name, ptr::null_mut()) })
        };
        let number = |name| {
            // SAFETY: as above.
            unsafe { SDL_GetNumberProperty(props, name, 0) }
        };
        NativeHandles {
            metal_view: self.metal_view,
            win32_hwnd: pointer(SDL_PROP_WINDOW_WIN32_HWND_POINTER),
            win32_hinstance: pointer(SDL_PROP_WINDOW_WIN32_INSTANCE_POINTER),
            wayland_display: pointer(SDL_PROP_WINDOW_WAYLAND_DISPLAY_POINTER),
            wayland_surface: pointer(SDL_PROP_WINDOW_WAYLAND_SURFACE_POINTER),
            x11_display: pointer(SDL_PROP_WINDOW_X11_DISPLAY_POINTER),
            x11_screen: number(SDL_PROP_WINDOW_X11_SCREEN_NUMBER),
            x11_window: number(SDL_PROP_WINDOW_X11_WINDOW_NUMBER),
            android_window: None,
        }
    }
}

impl Drop for WindowInner {
    fn drop(&mut self) {
        // SAFETY: SDL_WasInit has no preconditions.
        let video_alive = unsafe { SDL_WasInit(SDL_INIT_VIDEO) }.0 & SDL_INIT_VIDEO.0 != 0;
        if !video_alive {
            // SDL_Quit already destroyed every window and view; touching them now would
            // be a use-after-free. Only possible if the app leaked the window past quit.
            return;
        }
        if let Some(view) = self.metal_view {
            // SAFETY: the view was created for this window and is destroyed exactly once,
            // before its window.
            unsafe { SDL_Metal_DestroyView(view.as_ptr()) };
        }
        // SAFETY: the last owner is going away and nothing uses the window after this.
        unsafe { SDL_DestroyWindow(self.raw.as_ptr()) };
    }
}

/// The native objects a GPU surface is created from, kept alive while this value exists.
///
/// Take one with [`Window::surface_handle`], create the surface from it, and keep it for
/// exactly as long as that surface: drop the surface first, then the handle. Never keep
/// a raw handle obtained from it any longer than the `SurfaceHandle` itself.
///
/// Building the renderer's `SurfaceTarget` from it:
///
/// ```no_run
/// # use std::{ffi::c_void, ptr::NonNull};
/// # use sapphire_platform::SurfaceHandle;
/// # use sapphire_platform::raw_window_handle::{
/// #     HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
/// # };
/// # enum SurfaceTarget {
/// #     CoreAnimationLayer(NonNull<c_void>),
/// #     Raw { window: RawWindowHandle, display: RawDisplayHandle },
/// # }
/// fn surface_target(handle: &SurfaceHandle) -> Result<SurfaceTarget, HandleError> {
///     Ok(match handle.metal_layer() {
///         Some(layer) => SurfaceTarget::CoreAnimationLayer(layer),
///         None => SurfaceTarget::Raw {
///             window: handle.window_handle()?.as_raw(),
///             display: handle.display_handle()?.as_raw(),
///         },
///     })
/// }
/// ```
///
/// - **Apple:** [`metal_layer`](Self::metal_layer) is the `CAMetalLayer` of the SDL Metal
///   view created with the window. It is the same layer for the window's whole life,
///   including across background / foreground. The handle traits report that view as an
///   AppKit / UIKit handle, for tools that want a view.
/// - **Windows, Linux:** `metal_layer` is `None`; the traits report the Win32
///   `HWND` / `HINSTANCE`, the Wayland `wl_surface` / `wl_display`, or the Xlib `Window` /
///   `Display` (SDL picks Wayland or X11 at startup).
/// - **Android:** the window handle is the `ANativeWindow`, and the `SurfaceHandle`
///   holds its own reference to it. Android still destroys the window's surface on every
///   trip to the background (presenting then fails as lost), but the pointer stays valid,
///   so wgpu never touches freed memory. Drop the surface and this handle on
///   `WillEnterBackground`, and take a new handle on `DidEnterForeground`: the OS hands
///   the app a different `ANativeWindow` each time (see
///   [`LifecycleEvent`](crate::LifecycleEvent)).
///
/// The handle also keeps the SDL window itself alive, so the native objects stay valid
/// even if the [`Window`] is dropped first.
#[derive(Debug)]
pub struct SurfaceHandle {
    window: RawWindowHandle,
    display: RawDisplayHandle,
    metal_layer: Option<NonNull<c_void>>,
    _android_window: AndroidWindowRef,
    _owner: Rc<WindowInner>,
}

impl SurfaceHandle {
    /// The `CAMetalLayer` to render into on Apple platforms; `None` elsewhere.
    pub fn metal_layer(&self) -> Option<NonNull<c_void>> {
        self.metal_layer
    }
}

impl HasWindowHandle for SurfaceHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: `self` keeps every object the handle names alive: the SDL window (and
        // with it the Metal view, HWND or wl_surface) through `_owner`, and on Android the
        // ANativeWindow through its own reference.
        Ok(unsafe { WindowHandle::borrow_raw(self.window) })
    }
}

impl HasDisplayHandle for SurfaceHandle {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: display connections belong to SDL's video subsystem, which shuts down
        // only after the app (and so every handle it owns) has been dropped.
        Ok(unsafe { DisplayHandle::borrow_raw(self.display) })
    }
}

/// A reference of our own to Android's `ANativeWindow`, so the window can't be freed
/// while a GPU surface uses it. Always empty on other targets.
#[derive(Debug)]
struct AndroidWindowRef(Option<NonNull<c_void>>);

impl AndroidWindowRef {
    /// Reads the window's `ANativeWindow` and takes a reference to it.
    #[cfg(target_os = "android")]
    fn acquire(props: SDL_PropertiesID) -> Self {
        use sdl3_sys::properties::{SDL_LockProperties, SDL_UnlockProperties};
        use sdl3_sys::video::SDL_PROP_WINDOW_ANDROID_WINDOW_POINTER;

        // SDL's `onNativeSurfaceDestroyed` runs on Android's UI thread: it clears this
        // property under the property lock and only then releases SDL's own reference.
        // Holding the same lock across read and acquire means we either read null or
        // take our reference while SDL's is still held.
        // SAFETY: `props` belongs to a live window.
        if !unsafe { SDL_LockProperties(props) } {
            return Self(None);
        }
        // SAFETY: as above; the lock is held.
        let window = NonNull::new(unsafe {
            SDL_GetPointerProperty(props, SDL_PROP_WINDOW_ANDROID_WINDOW_POINTER, ptr::null_mut())
        });
        if let Some(window) = window {
            // SAFETY: SDL still holds its reference (see above), so the window is alive.
            unsafe { ndk::ANativeWindow_acquire(window.as_ptr()) };
        }
        // SAFETY: locked above.
        unsafe { SDL_UnlockProperties(props) };
        Self(window)
    }

    #[cfg(not(target_os = "android"))]
    fn acquire(_props: SDL_PropertiesID) -> Self {
        Self(None)
    }

    fn get(&self) -> Option<NonNull<c_void>> {
        self.0
    }
}

#[cfg(target_os = "android")]
impl Drop for AndroidWindowRef {
    fn drop(&mut self) {
        if let Some(window) = self.0 {
            // SAFETY: the reference was taken in `acquire` and is released exactly once.
            unsafe { ndk::ANativeWindow_release(window.as_ptr()) };
        }
    }
}

#[cfg(target_os = "android")]
mod ndk {
    use std::ffi::c_void;

    #[link(name = "android")]
    unsafe extern "C" {
        pub fn ANativeWindow_acquire(window: *mut c_void);
        pub fn ANativeWindow_release(window: *mut c_void);
    }
}

fn to_size(width: c_int, height: c_int) -> Size {
    Size { width: u32::try_from(width).unwrap_or(0), height: u32::try_from(height).unwrap_or(0) }
}

/// The window-system family of the compilation target, deciding how SDL's window
/// properties map to raw-window-handle types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowSystem {
    AppKit,
    UiKit,
    Win32,
    Android,
    /// Linux and other free Unixes: Wayland or X11, chosen by SDL at runtime.
    Unix,
}

impl WindowSystem {
    const CURRENT: Self = if cfg!(target_os = "macos") {
        Self::AppKit
    } else if cfg!(target_os = "ios") {
        Self::UiKit
    } else if cfg!(target_os = "windows") {
        Self::Win32
    } else if cfg!(target_os = "android") {
        Self::Android
    } else {
        Self::Unix
    };
}

/// Snapshot of the native objects behind a window. Kept separate from SDL so the
/// mapping to raw-window-handle can be tested on any host.
#[derive(Clone, Copy, Debug, Default)]
struct NativeHandles {
    metal_view: Option<NonNull<c_void>>,
    win32_hwnd: Option<NonNull<c_void>>,
    win32_hinstance: Option<NonNull<c_void>>,
    wayland_display: Option<NonNull<c_void>>,
    wayland_surface: Option<NonNull<c_void>>,
    x11_display: Option<NonNull<c_void>>,
    x11_screen: i64,
    x11_window: i64,
    android_window: Option<NonNull<c_void>>,
}

impl NativeHandles {
    fn x11_window(&self) -> Option<c_ulong> {
        c_ulong::try_from(self.x11_window).ok().filter(|&xid| xid != 0)
    }

    fn window_handle(&self, system: WindowSystem) -> Result<RawWindowHandle, HandleError> {
        match system {
            WindowSystem::AppKit => {
                let view = self.metal_view.ok_or(HandleError::Unavailable)?;
                Ok(AppKitWindowHandle::new(view).into())
            }
            WindowSystem::UiKit => {
                let view = self.metal_view.ok_or(HandleError::Unavailable)?;
                Ok(UiKitWindowHandle::new(view).into())
            }
            WindowSystem::Win32 => {
                let hwnd = self.win32_hwnd.and_then(|p| NonZeroIsize::new(p.as_ptr().addr().cast_signed()));
                let mut handle = Win32WindowHandle::new(hwnd.ok_or(HandleError::Unavailable)?);
                handle.hinstance =
                    self.win32_hinstance.and_then(|p| NonZeroIsize::new(p.as_ptr().addr().cast_signed()));
                Ok(handle.into())
            }
            WindowSystem::Android => {
                // None while the app is in the background (see `LifecycleEvent`).
                let window = self.android_window.ok_or(HandleError::Unavailable)?;
                Ok(AndroidNdkWindowHandle::new(window).into())
            }
            WindowSystem::Unix => {
                if let Some(surface) = self.wayland_surface {
                    Ok(WaylandWindowHandle::new(surface).into())
                } else if let Some(xid) = self.x11_window() {
                    Ok(XlibWindowHandle::new(xid).into())
                } else {
                    // An SDL video driver with no raw-window-handle equivalent
                    // (offscreen, KMSDRM).
                    Err(HandleError::NotSupported)
                }
            }
        }
    }

    fn display_handle(&self, system: WindowSystem) -> Result<RawDisplayHandle, HandleError> {
        match system {
            WindowSystem::AppKit => Ok(AppKitDisplayHandle::new().into()),
            WindowSystem::UiKit => Ok(UiKitDisplayHandle::new().into()),
            WindowSystem::Win32 => Ok(WindowsDisplayHandle::new().into()),
            WindowSystem::Android => Ok(AndroidDisplayHandle::new().into()),
            WindowSystem::Unix => {
                if self.wayland_surface.is_some() {
                    let display = self.wayland_display.ok_or(HandleError::Unavailable)?;
                    Ok(WaylandDisplayHandle::new(display).into())
                } else if self.x11_window().is_some() {
                    let screen = c_int::try_from(self.x11_screen).unwrap_or(0);
                    Ok(XlibDisplayHandle::new(self.x11_display, screen).into())
                } else {
                    Err(HandleError::NotSupported)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(addr: usize) -> Option<NonNull<c_void>> {
        NonNull::new(ptr::without_provenance_mut(addr))
    }

    #[test]
    fn apple_handles_use_the_metal_view() {
        let native = NativeHandles { metal_view: fake(0x10), ..Default::default() };
        assert!(matches!(
            native.window_handle(WindowSystem::AppKit),
            Ok(RawWindowHandle::AppKit(h)) if h.ns_view == fake(0x10).unwrap()
        ));
        assert!(matches!(native.window_handle(WindowSystem::UiKit), Ok(RawWindowHandle::UiKit(_))));
        assert!(matches!(native.display_handle(WindowSystem::UiKit), Ok(RawDisplayHandle::UiKit(_))));
    }

    #[test]
    fn win32_handle_carries_hinstance() {
        let native =
            NativeHandles { win32_hwnd: fake(0x1234), win32_hinstance: fake(0x400000), ..Default::default() };
        let Ok(RawWindowHandle::Win32(handle)) = native.window_handle(WindowSystem::Win32) else {
            panic!("expected a Win32 handle")
        };
        assert_eq!(handle.hwnd.get(), 0x1234);
        assert_eq!(handle.hinstance.map(NonZeroIsize::get), Some(0x400000));
        assert!(matches!(native.display_handle(WindowSystem::Win32), Ok(RawDisplayHandle::Windows(_))));
    }

    #[test]
    fn unix_prefers_wayland_then_x11() {
        let wayland =
            NativeHandles { wayland_surface: fake(0x20), wayland_display: fake(0x30), ..Default::default() };
        assert!(matches!(wayland.window_handle(WindowSystem::Unix), Ok(RawWindowHandle::Wayland(_))));
        assert!(matches!(
            wayland.display_handle(WindowSystem::Unix),
            Ok(RawDisplayHandle::Wayland(h)) if h.display == fake(0x30).unwrap()
        ));

        let x11 = NativeHandles {
            x11_display: fake(0x40),
            x11_screen: 1,
            x11_window: 0x5000,
            ..Default::default()
        };
        assert!(matches!(
            x11.window_handle(WindowSystem::Unix),
            Ok(RawWindowHandle::Xlib(h)) if h.window == 0x5000
        ));
        assert!(matches!(
            x11.display_handle(WindowSystem::Unix),
            Ok(RawDisplayHandle::Xlib(h)) if h.screen == 1 && h.display == fake(0x40)
        ));

        let headless = NativeHandles::default();
        assert!(matches!(headless.window_handle(WindowSystem::Unix), Err(HandleError::NotSupported)));
    }

    #[test]
    fn android_window_is_unavailable_while_backgrounded() {
        let backgrounded = NativeHandles::default();
        assert!(matches!(backgrounded.window_handle(WindowSystem::Android), Err(HandleError::Unavailable)));
        let foreground = NativeHandles { android_window: fake(0x50), ..Default::default() };
        assert!(matches!(
            foreground.window_handle(WindowSystem::Android),
            Ok(RawWindowHandle::AndroidNdk(_))
        ));
        assert!(matches!(foreground.display_handle(WindowSystem::Android), Ok(RawDisplayHandle::Android(_))));
    }
}
