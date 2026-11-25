//! Declarations of the AX and CoreGraphics C functions that the backend calls.

use std::ffi::c_void;

use core_foundation::array::CFArrayRef;
use core_foundation::base::{CFTypeID, CFTypeRef};
use core_foundation::dictionary::CFDictionaryRef;
use core_foundation::runloop::CFRunLoopSourceRef;
use core_foundation::string::CFStringRef;

pub type AXUIElementRef = *const c_void;
pub type AXObserverRef = *const c_void;
pub type AXError = i32;
pub const AX_SUCCESS: AXError = 0;
/// The element is gone. For example, the application's process exited.
pub const AX_INVALID_UI_ELEMENT: AXError = -25202;
/// The application didn't answer. For example, the application is launching or hangs.
pub const AX_CANNOT_COMPLETE: AXError = -25204;
/// This process may not use the AX API.
pub const AX_API_DISABLED: AXError = -25211;
/// The element does not post the notification.
pub const AX_NOTIFICATION_UNSUPPORTED: AXError = -25207;
/// The element doesn't have the attribute.
pub const AX_ATTRIBUTE_UNSUPPORTED: AXError = -25205;
/// The element has the attribute, without a value.
pub const AX_NO_VALUE: AXError = -25212;
pub const AX_VALUE_CGPOINT: u32 = 1;
pub const AX_VALUE_CGSIZE: u32 = 2;
/// The type of an `AXValue` that holds an `AXError`.
pub const AX_VALUE_AX_ERROR: u32 = 5;
pub const CG_WINDOW_LIST_ON_SCREEN_ONLY: u32 = 1;
pub const CG_WINDOW_LIST_EXCLUDE_DESKTOP: u32 = 16;

pub type AXObserverCallback =
    extern "C" fn(AXObserverRef, AXUIElementRef, CFStringRef, *mut c_void);

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub static kAXTrustedCheckOptionPrompt: CFStringRef;
    pub fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    pub fn AXUIElementGetTypeID() -> CFTypeID;
    pub fn AXValueGetTypeID() -> CFTypeID;
    pub fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    pub fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
    pub fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, seconds: f32) -> AXError;
    pub fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    pub fn AXUIElementCopyMultipleAttributeValues(
        element: AXUIElementRef,
        attributes: CFArrayRef,
        options: u32,
        values: *mut CFArrayRef,
    ) -> AXError;
    pub fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    pub fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut c_void) -> u8;
    pub fn AXObserverCreate(
        pid: i32,
        callback: AXObserverCallback,
        observer: *mut AXObserverRef,
    ) -> AXError;
    pub fn AXObserverAddNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
        context: *mut c_void,
    ) -> AXError;
    pub fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> CFRunLoopSourceRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub static kCGWindowOwnerPID: CFStringRef;
    pub static kCGWindowLayer: CFStringRef;
    pub fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
    pub fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}
