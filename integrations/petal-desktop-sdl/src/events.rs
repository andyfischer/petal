//! Reading SDL's event queue safely under sdl2-compat, and painting while a
//! window is being resized.
//!
//! Homebrew's `sdl2` formula installs **sdl2-compat**, which implements the
//! SDL2 API on top of SDL3. Besides the SDL2 `SDL_WINDOWEVENT`s (`0x200`) it
//! makes from SDL3's window events, it also queues some of those SDL3 events
//! as they are, numbered `0x202..=0x20F`, where real SDL2 defines nothing.
//! rust-sdl2 converts an event's id with an enum transmute, so one of those
//! ids aborts the process ("trying to construct an enum from an invalid value
//! 0x207") the first time a window is shown.
//!
//! They can't be disabled with `SDL_EventState`: in SDL3 that also stops
//! sdl2-compat from seeing them, and so from making the `SDL_WINDOWEVENT`s,
//! resizes included. [`next_event`] and [`poll_iter`] read the queue raw and
//! skip those ids instead.

use std::ffi::c_void;
use std::mem::MaybeUninit;

use sdl2::event::Event;
use sdl2::sys;

/// Whether `event_type` is one of SDL3's own window events, which
/// sdl2-compat queues next to the SDL2 events it makes from them.
pub fn is_untranslated(event_type: u32) -> bool {
    (0x202..=0x20F).contains(&event_type)
}

/// The next queued event, skipping the ids rust-sdl2 can't convert (see the
/// module docs). Use it in place of `EventPump::poll_event`, keeping the
/// pump alive for the event subsystem.
pub fn next_event() -> Option<Event> {
    loop {
        let mut raw = MaybeUninit::<sys::SDL_Event>::uninit();
        if unsafe { sys::SDL_PollEvent(raw.as_mut_ptr()) } == 0 {
            return None;
        }
        let raw = unsafe { raw.assume_init() };
        if !is_untranslated(unsafe { raw.type_ }) {
            return Some(Event::from_ll(raw));
        }
    }
}

/// Every queued event, as [`next_event`] reads them: the replacement for
/// `EventPump::poll_iter`. Borrowing the pump keeps the event subsystem up.
pub fn poll_iter(_pump: &mut sdl2::EventPump) -> impl Iterator<Item = Event> + '_ {
    std::iter::from_fn(next_event)
}

/// Calls `on_resize` each time a window changes size, from inside SDL rather
/// than from the main loop: for painting while a window is being resized.
///
/// macOS runs its own event loop for as long as a window edge is dragged, so
/// the app's loop doesn't get a turn and the window shows its last frame
/// stretched to fit. SDL still hands each size change to its event watches as
/// it happens, and a frame painted from here is shown at the new size.
///
/// The watch runs whenever SDL queues an event, which includes inside
/// [`next_event`]; keep the app's state somewhere both can reach (a
/// `RefCell` the loop doesn't hold borrowed while it polls, and the callback
/// `try_borrow_mut`s).
///
/// This is SDL's own watch rather than `EventSubsystem::add_event_watch`.
/// rust-sdl2 hands that watch each event converted with `Event::from_ll`,
/// which takes ownership of a dropped file's name and frees it; the same
/// event, still in the queue, then gives the main loop's poll a freed string
/// (and frees it again). This watch reads only the event's type, never
/// converting it.
pub struct ResizeWatch<'a> {
    callback: Box<Box<dyn FnMut() + 'a>>,
}

impl<'a> ResizeWatch<'a> {
    /// Start calling `on_resize`; it stops when the watch is dropped.
    pub fn new(on_resize: impl FnMut() + 'a) -> ResizeWatch<'a> {
        let mut watch = ResizeWatch {
            callback: Box::new(Box::new(on_resize)),
        };
        // The pointer is to the outer box's heap slot, so moving `watch` out
        // below leaves it valid.
        let data = watch.data();
        unsafe { sys::SDL_AddEventWatch(Some(on_event), data) };
        watch
    }

    fn data(&mut self) -> *mut c_void {
        &mut *self.callback as *mut Box<dyn FnMut() + 'a> as *mut c_void
    }
}

impl Drop for ResizeWatch<'_> {
    fn drop(&mut self) {
        let data = self.data();
        unsafe { sys::SDL_DelEventWatch(Some(on_event), data) };
    }
}

/// Whether a raw event is a window's size changing.
fn is_size_change(event: &sys::SDL_Event) -> bool {
    unsafe {
        event.type_ == sys::SDL_EventType::SDL_WINDOWEVENT as u32
            && event.window.event == sys::SDL_WindowEventID::SDL_WINDOWEVENT_SIZE_CHANGED as u8
    }
}

extern "C" fn on_event(data: *mut c_void, event: *mut sys::SDL_Event) -> i32 {
    if is_size_change(unsafe { &*event }) {
        let callback = unsafe { &mut *(data as *mut Box<dyn FnMut()>) };
        callback();
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sdl3_window_ids_are_untranslated() {
        assert!(!is_untranslated(0x200)); // SDL_WINDOWEVENT
        assert!(!is_untranslated(0x201)); // SDL_SYSWMEVENT
        assert!(is_untranslated(0x202));
        assert!(is_untranslated(0x207));
        assert!(is_untranslated(0x20F));
        assert!(!is_untranslated(0x300)); // SDL_KEYDOWN
        assert!(!is_untranslated(0x1000)); // SDL_DROPFILE
    }

    #[test]
    fn a_size_change_is_a_window_event_with_that_id() {
        let mut e: sys::SDL_Event = unsafe { std::mem::zeroed() };
        e.type_ = sys::SDL_EventType::SDL_WINDOWEVENT as u32;
        e.window.event = sys::SDL_WindowEventID::SDL_WINDOWEVENT_SIZE_CHANGED as u8;
        assert!(is_size_change(&e));
        e.window.event = sys::SDL_WindowEventID::SDL_WINDOWEVENT_MOVED as u8;
        assert!(!is_size_change(&e));
        e.type_ = sys::SDL_EventType::SDL_DROPFILE as u32;
        assert!(!is_size_change(&e));
    }
}
