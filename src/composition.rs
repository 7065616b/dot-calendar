//! A small UI-thread-only DirectComposition adapter for existing Win32 windows.
//! COM ABI slots follow dcomp.h / the Windows metadata, including the overloaded
//! method order (animation pointer before scalar). No new crate/runtime is used.
//! Only visual offsets, scale and opacity animate. HWND geometry is never tweened.

use crate::ui_design::Motion;
use std::{
    ffi::c_void,
    marker::PhantomData,
    ptr::null_mut,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

static NEXT_SOURCE: AtomicUsize = AtomicUsize::new(1);
fn source_property() -> Vec<u16> {
    "DotCalendar.CompositionSource"
        .encode_utf16()
        .chain(Some(0))
        .collect()
}
pub unsafe fn source_stamp(hwnd: HWND) -> usize {
    GetPropW(hwnd, source_property().as_ptr()) as usize
}
use windows_sys::{
    core::GUID,
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        UI::WindowsAndMessaging::*,
    },
};

#[link(name = "dcomp")]
extern "system" {
    fn DCompositionCreateDevice(
        device: *mut c_void,
        iid: *const GUID,
        output: *mut *mut c_void,
    ) -> i32;
}
const DEVICE_IID: GUID = GUID {
    data1: 0xc37ea93a,
    data2: 0xe7aa,
    data3: 0x450d,
    data4: [0xb1, 0x6f, 0x97, 0x46, 0xcb, 0x04, 0x07, 0xf3],
};

struct Com(*mut c_void, PhantomData<Rc<()>>);
impl Com {
    unsafe fn method<T: Copy>(&self, slot: usize) -> T {
        // All pointers originate from successful fixed-IID Windows COM calls.
        // T is the corresponding Windows ABI function signature at each callsite.
        debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
        let vtable = *(self.0 as *const *const *const c_void);
        std::mem::transmute_copy(&*vtable.add(slot))
    }
    unsafe fn object(&self, slot: usize) -> Result<Self, String> {
        let call: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
            self.method(slot);
        let mut pointer = null_mut();
        check(call(self.0, &mut pointer))?;
        if pointer.is_null() {
            return Err("DirectComposition returned a null object".into());
        }
        Ok(Self(pointer, PhantomData))
    }
    unsafe fn scalar(&self, slot: usize, value: f32) -> Result<(), String> {
        let call: unsafe extern "system" fn(*mut c_void, f32) -> i32 = self.method(slot);
        check(call(self.0, value))
    }
    unsafe fn pointer(&self, slot: usize, value: *mut c_void) -> Result<(), String> {
        let call: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32 = self.method(slot);
        check(call(self.0, value))
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            let release: unsafe extern "system" fn(*mut c_void) -> u32 = self.method(2);
            release(self.0);
        }
    }
}
fn check(hr: i32) -> Result<(), String> {
    if hr < 0 {
        Err(format!("DirectComposition 0x{:08X}", hr as u32))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    Restore,
    Destroy,
    Hold,
}

struct Layer {
    hwnd: HWND,
    visual: Com,
    _surface: Com,
    _scale: Com,
    _opacity: Com,
    old_style: isize,
    old_alpha: u8,
    old_key: u32,
    old_flags: u32,
    deadline: Option<Instant>,
    finish: Finish,
    stamp: usize,
}
impl Layer {
    unsafe fn restore(&self) {
        if IsWindow(self.hwnd) == 0 || source_stamp(self.hwnd) != self.stamp {
            return;
        }
        RemovePropW(self.hwnd, source_property().as_ptr());
        let no: i32 = 0;
        DwmSetWindowAttribute(self.hwnd, DWMWA_CLOAK as u32, &no as *const _ as _, 4);
        SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, self.old_style);
        if self.old_style & WS_EX_LAYERED as isize != 0 && self.old_flags != 0 {
            SetLayeredWindowAttributes(self.hwnd, self.old_key, self.old_alpha, self.old_flags);
        }
        RedrawWindow(
            self.hwnd,
            std::ptr::null(),
            null_mut(),
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
        );
    }
}

pub struct Compositor {
    device: Com,
    _target: Com,
    root: Com,
    host: HWND,
    layers: Vec<Layer>,
    pub submitted: u64,
    pub completed: u64,
}
impl Compositor {
    pub unsafe fn new(host: HWND) -> Result<Self, String> {
        let mut pointer = null_mut();
        check(DCompositionCreateDevice(
            null_mut(),
            &DEVICE_IID,
            &mut pointer,
        ))?;
        if pointer.is_null() {
            return Err("DirectComposition device missing".into());
        }
        let device = Com(pointer, PhantomData);
        let make_target: unsafe extern "system" fn(
            *mut c_void,
            HWND,
            i32,
            *mut *mut c_void,
        ) -> i32 = device.method(6);
        let mut pointer = null_mut();
        check(make_target(device.0, host, 1, &mut pointer))?;
        if pointer.is_null() {
            return Err("DirectComposition target missing".into());
        }
        let target = Com(pointer, PhantomData);
        let root = device.object(7)?;
        target.pointer(3, root.0)?;
        let result = Self {
            device,
            _target: target,
            root,
            host,
            layers: vec![],
            submitted: 0,
            completed: 0,
        };
        result.commit()?;
        Ok(result)
    }

    unsafe fn commit(&self) -> Result<(), String> {
        let call: unsafe extern "system" fn(*mut c_void) -> i32 = self.device.method(3);
        check(call(self.device.0))
    }

    unsafe fn animate_property(
        &self,
        target: &Com,
        animated_slot: usize,
        scalar_slot: usize,
        from: f32,
        to: f32,
        motion: Motion,
    ) -> Result<(), String> {
        if (to - from).abs() < 0.00001 {
            return target.scalar(scalar_slot, to);
        }
        let animation = self.device.object(25)?;
        let cubic: unsafe extern "system" fn(*mut c_void, f64, f32, f32, f32, f32) -> i32 =
            animation.method(5);
        for part in motion.curve.segments(from, to, motion.milliseconds) {
            let [a, b, c, d] = part.coefficients;
            check(cubic(animation.0, part.offset, a, b, c, d))?;
        }
        let end: unsafe extern "system" fn(*mut c_void, f64, f32) -> i32 = animation.method(8);
        check(end(animation.0, motion.milliseconds as f64 / 1000.0, to))?;
        target.pointer(animated_slot, animation.0)
    }

    /// Called outside any App RefCell borrow: repaint can synchronously call the
    /// native owner's WM_DRAWITEM. One source may have only one live visual.
    pub unsafe fn start(
        &mut self,
        hwnd: HWND,
        motion: Motion,
        dpi: u32,
        finish: Finish,
    ) -> Result<(), String> {
        self.cancel(hwnd);
        if IsWindow(hwnd) == 0 {
            return Err("Animation source window no longer exists".into());
        }
        let old_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let stamp = NEXT_SOURCE.fetch_add(1, Ordering::Relaxed).max(1);
        if SetPropW(hwnd, source_property().as_ptr(), stamp as HANDLE) == 0 {
            return Err("Cannot identify animation source".into());
        }
        let (mut old_key, mut old_alpha, mut old_flags) = (0, 255, 0);
        if old_style & WS_EX_LAYERED as isize != 0 {
            GetLayeredWindowAttributes(hwnd, &mut old_key, &mut old_alpha, &mut old_flags);
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, old_style | WS_EX_LAYERED as isize);
        if old_flags == 0 && SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA) == 0 {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, old_style);
            RemovePropW(hwnd, source_property().as_ptr());
            return Err(format!(
                "Layered window: {}",
                std::io::Error::last_os_error()
            ));
        }
        RedrawWindow(
            hwnd,
            std::ptr::null(),
            null_mut(),
            RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
        );
        let result = (|| {
            let visual = self.device.object(7)?;
            let mut surface = null_mut();
            let make_surface: unsafe extern "system" fn(
                *mut c_void,
                HWND,
                *mut *mut c_void,
            ) -> i32 = self.device.method(11);
            check(make_surface(self.device.0, hwnd, &mut surface))?;
            if surface.is_null() {
                return Err("Composition surface missing".into());
            }
            let surface = Com(surface, PhantomData);
            visual.pointer(15, surface.0)?;
            let scale = self.device.object(13)?;
            let opacity = self.device.object(23)?;
            let mut bounds: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut bounds);
            let mut origin = POINT {
                x: bounds.left,
                y: bounds.top,
            };
            ScreenToClient(self.host, &mut origin);
            let factor = dpi as f32 / 96.0;
            self.animate_property(
                &visual,
                3,
                4,
                origin.x as f32 + motion.from.x * factor,
                origin.x as f32 + motion.to.x * factor,
                motion,
            )?;
            self.animate_property(
                &visual,
                5,
                6,
                origin.y as f32 + motion.from.y * factor,
                origin.y as f32 + motion.to.y * factor,
                motion,
            )?;
            scale.scalar(8, (bounds.right - bounds.left) as f32 / 2.0)?;
            scale.scalar(10, (bounds.bottom - bounds.top) as f32 / 2.0)?;
            self.animate_property(&scale, 3, 4, motion.from.scale, motion.to.scale, motion)?;
            self.animate_property(&scale, 5, 6, motion.from.scale, motion.to.scale, motion)?;
            visual.pointer(7, scale.0)?;
            self.animate_property(
                &opacity,
                3,
                4,
                motion.from.opacity,
                motion.to.opacity,
                motion,
            )?;
            visual.pointer(10, opacity.0)?;
            let add: unsafe extern "system" fn(*mut c_void, *mut c_void, i32, *mut c_void) -> i32 =
                self.root.method(16);
            check(add(self.root.0, visual.0, 1, null_mut()))?;
            let yes: i32 = 1;
            if let Err(error) = check(DwmSetWindowAttribute(
                hwnd,
                DWMWA_CLOAK as u32,
                &yes as *const _ as _,
                4,
            )) {
                let _ = self.root.pointer(17, visual.0);
                return Err(error);
            }
            let layer = Layer {
                hwnd,
                visual,
                _surface: surface,
                _scale: scale,
                _opacity: opacity,
                old_style,
                old_alpha,
                old_key,
                old_flags,
                deadline: Some(
                    Instant::now() + Duration::from_millis(motion.milliseconds as u64 + 20),
                ),
                finish,
                stamp,
            };
            if let Err(error) = self.commit() {
                layer.restore();
                let _ = self.root.pointer(17, layer.visual.0);
                return Err(error);
            }
            self.layers.push(layer);
            self.submitted += 1;
            Ok(())
        })();
        if result.is_err() {
            RemovePropW(hwnd, source_property().as_ptr());
            let no: i32 = 0;
            DwmSetWindowAttribute(hwnd, DWMWA_CLOAK as u32, &no as *const _ as _, 4);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, old_style);
            if old_flags != 0 {
                SetLayeredWindowAttributes(hwnd, old_key, old_alpha, old_flags);
            }
            let _ = self.commit();
        }
        result
    }

    pub unsafe fn cancel(&mut self, hwnd: HWND) {
        if let Some(index) = self.layers.iter().position(|layer| layer.hwnd == hwnd) {
            let layer = self.layers.remove(index);
            let _ = self.root.pointer(17, layer.visual.0);
            layer.restore();
            let _ = self.commit();
        }
    }

    /// A child is already being destroyed. Do not restore its style or touch a
    /// subsequently reused HWND; only detach that exact source generation.
    pub unsafe fn forget(&mut self, hwnd: HWND, stamp: usize) {
        if let Some(index) = self
            .layers
            .iter()
            .position(|layer| layer.hwnd == hwnd && layer.stamp == stamp)
        {
            let layer = self.layers.remove(index);
            let _ = self.root.pointer(17, layer.visual.0);
            let _ = self.commit();
        }
    }

    pub unsafe fn finish_due(&mut self, all: bool) {
        let now = Instant::now();
        let mut i = 0;
        while i < self.layers.len() {
            if !all
                && self.layers[i]
                    .deadline
                    .is_none_or(|deadline| deadline > now)
            {
                i += 1;
                continue;
            }
            self.completed += 1;
            if !all && self.layers[i].finish == Finish::Hold {
                self.layers[i].deadline = None;
                i += 1;
                continue;
            }
            let layer = self.layers.remove(i);
            let _ = self.root.pointer(17, layer.visual.0);
            let owns_source = source_stamp(layer.hwnd) == layer.stamp;
            layer.restore();
            if owns_source && layer.finish == Finish::Destroy && IsWindow(layer.hwnd) != 0 {
                DestroyWindow(layer.hwnd);
            }
        }
        let _ = self.commit();
    }

    pub fn next_deadline_ms(&self) -> Option<u32> {
        self.layers
            .iter()
            .filter_map(|layer| layer.deadline)
            .min()
            .map(|deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .clamp(1, 1000) as u32
            })
    }
    #[cfg(debug_assertions)]
    pub fn count(&self) -> usize {
        self.layers.len()
    }
}
impl Drop for Compositor {
    fn drop(&mut self) {
        unsafe {
            self.finish_due(true);
        }
    }
}
