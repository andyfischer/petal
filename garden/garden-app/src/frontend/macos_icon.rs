use std::ffi::c_void;

use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::{MainThreadMarker, NSData};

const ICON_PNG: &[u8] = include_bytes!("../../assets/macos/GardenIcon.png");

pub fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    let data =
        unsafe { NSData::dataWithBytes_length(ICON_PNG.as_ptr().cast::<c_void>(), ICON_PNG.len()) };
    let Some(image) = NSImage::initWithData(mtm.alloc(), &data) else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);

    unsafe {
        app.setApplicationIconImage(Some(&image));
    }
}
