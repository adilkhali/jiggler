//! Thin, thread-safe wrappers over the macOS C APIs Jiggler relies on.

use std::f64::consts::TAU;
use std::ffi::c_void;
use std::thread::sleep;
use std::time::Duration;

use objc2::rc::Retained;
use objc2_foundation::{NSDictionary, NSNumber, NSString};

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

type CFTypeRef = *const c_void;

const HID_SYSTEM_STATE: i32 = 1; // kCGEventSourceStateHIDSystemState
const HID_EVENT_TAP: u32 = 0; // kCGHIDEventTap
const MOUSE_MOVED: u32 = 5; // kCGEventMouseMoved
const LEFT_BUTTON: u32 = 0; // kCGMouseButtonLeft (ignored for moves)
const ANY_INPUT_EVENT: u32 = !0; // kCGAnyInputEventType
const ASSERTION_LEVEL_ON: u32 = 255; // kIOPMAssertionLevelOn

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceCreate(state_id: i32) -> CFTypeRef;
    fn CGEventCreate(source: CFTypeRef) -> CFTypeRef;
    fn CGEventGetLocation(event: CFTypeRef) -> CGPoint;
    fn CGEventCreateMouseEvent(
        source: CFTypeRef,
        mouse_type: u32,
        position: CGPoint,
        button: u32,
    ) -> CFTypeRef;
    fn CGEventPost(tap: u32, event: CFTypeRef);
    fn CGEventSourceSecondsSinceLastEventType(state_id: i32, event_type: u32) -> f64;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    static kAXTrustedCheckOptionPrompt: CFTypeRef;
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: CFTypeRef,
        level: u32,
        name: CFTypeRef,
        assertion_id: *mut u32,
    ) -> i32;
    fn IOPMAssertionRelease(assertion_id: u32) -> i32;
}

/// Owned CoreFoundation object, released on drop.
struct CfOwned(CFTypeRef);

impl CfOwned {
    fn new(ptr: CFTypeRef) -> Option<Self> {
        (!ptr.is_null()).then_some(Self(ptr))
    }
}

impl Drop for CfOwned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}

/// Whether the app has been granted Accessibility access (required to post mouse events).
pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Shows the system "would like to control this computer" prompt (whose button opens
/// System Settings) and registers the app in the Accessibility list so the user only
/// has to flip its switch.
pub fn request_accessibility() -> bool {
    // SAFETY: the constant is an immutable CFString, toll-free bridged to NSString.
    let key: &NSString = unsafe { &*kAXTrustedCheckOptionPrompt.cast::<NSString>() };
    let value = NSNumber::new_bool(true);
    let options = NSDictionary::from_slices(&[key], &[&*value]);
    unsafe { AXIsProcessTrustedWithOptions(Retained::as_ptr(&options).cast()) }
}

/// Seconds since the last keyboard/mouse/trackpad input from any source.
pub fn idle_seconds() -> f64 {
    unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT) }
}

fn cursor_position() -> Option<CGPoint> {
    let event = CfOwned::new(unsafe { CGEventCreate(std::ptr::null()) })?;
    Some(unsafe { CGEventGetLocation(event.0) })
}

fn post_move(source: &CfOwned, pos: CGPoint) {
    let event = unsafe { CGEventCreateMouseEvent(source.0, MOUSE_MOVED, pos, LEFT_BUTTON) };
    if let Some(event) = CfOwned::new(event) {
        unsafe { CGEventPost(HID_EVENT_TAP, event.0) };
    }
}

/// Traces a few small circles from the current pointer position, then puts it back.
/// Stops early, leaving the pointer alone, if the user grabs the mouse mid-motion.
/// Blocks for roughly `loops * 0.36` seconds, so call it off the main thread.
pub fn jiggle(radius: f64, loops: u32) {
    const STEPS: u32 = 24;
    const STEP_DELAY: Duration = Duration::from_millis(15);
    const TOLERANCE: f64 = 3.0;

    let Some(source) = CfOwned::new(unsafe { CGEventSourceCreate(HID_SYSTEM_STATE) }) else {
        return;
    };
    let Some(origin) = cursor_position() else {
        return;
    };

    let mut expected = origin;
    for i in 1..=STEPS * loops {
        if let Some(now) = cursor_position() {
            let moved_by_user = (now.x - expected.x).hypot(now.y - expected.y) > TOLERANCE;
            if i > 1 && moved_by_user {
                return;
            }
        }
        let angle = TAU * f64::from(i) / f64::from(STEPS);
        expected = CGPoint {
            x: origin.x + radius * angle.cos() - radius,
            y: origin.y + radius * angle.sin(),
        };
        post_move(&source, expected);
        sleep(STEP_DELAY);
    }
    post_move(&source, origin);
}

/// Keeps the display (and therefore the system) from idle-sleeping while alive.
pub struct SleepAssertion(u32);

impl SleepAssertion {
    pub fn acquire(reason: &str) -> Option<Self> {
        let kind = NSString::from_str("PreventUserIdleDisplaySleep");
        let name = NSString::from_str(reason);
        let mut id = 0;
        let status = unsafe {
            IOPMAssertionCreateWithName(
                Retained::as_ptr(&kind).cast(),
                ASSERTION_LEVEL_ON,
                Retained::as_ptr(&name).cast(),
                &mut id,
            )
        };
        (status == 0).then_some(Self(id))
    }
}

impl Drop for SleepAssertion {
    fn drop(&mut self) {
        unsafe { IOPMAssertionRelease(self.0) };
    }
}
