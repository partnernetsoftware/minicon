//! AppKit event ownership for the browser child; product actions stay in the GUI.

use std::{cell::RefCell, ptr::NonNull, rc::Rc};

use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSView};
use wry::{WebView, WebViewExtMacOS};

use super::NativePageState;

pub(super) struct ShortcutMonitor(Retained<AnyObject>);

fn reserved_shortcut(
    character: char,
    control: bool,
    command: bool,
    shift: bool,
    option: bool,
) -> Option<char> {
    if option {
        return None;
    }
    if (control || command) && character == 'L' {
        return Some('L');
    }
    if command && !control && !shift {
        return match character {
            '[' => Some('<'),
            ']' => Some('>'),
            'R' => Some('R'),
            _ => None,
        };
    }
    (control && shift && "TODNWBHIPG[],".contains(character)).then_some(character)
}

impl ShortcutMonitor {
    pub(super) fn install(
        webview: &WebView,
        state: Rc<RefCell<NativePageState>>,
        wake: Rc<impl Fn() + 'static>,
    ) -> Result<Self, String> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "browser-native-main-thread-required".to_owned())?;
        let view: Retained<NSView> = webview.webview().into_super().into_super();
        let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // A local monitor runs synchronously on AppKit's main thread. The
            // borrowed event remains live until this callback returns.
            let original = event.as_ptr();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let event = unsafe { event.as_ref() };
                let Some(window) = view.window() else {
                    return original;
                };
                let Some(event_window) = event.window(mtm) else {
                    return original;
                };
                if !std::ptr::eq(&*window, &*event_window) || !window.isKeyWindow() {
                    return original;
                }
                let Some(responder) = window.firstResponder() else {
                    return original;
                };
                let Some(focused_view) = responder.downcast_ref::<NSView>() else {
                    return original;
                };
                if !focused_view.isDescendantOf(&view) {
                    return original;
                }
                let flags = event.modifierFlags();
                let control = flags.contains(NSEventModifierFlags::Control);
                let command = flags.contains(NSEventModifierFlags::Command);
                let shift = flags.contains(NSEventModifierFlags::Shift);
                let option = flags.contains(NSEventModifierFlags::Option);
                let character = match event.keyCode() {
                    // Shift changes these logical characters to {, } and <.
                    // Keep the same physical punctuation as the host chords.
                    0x21 => '[',
                    0x1e => ']',
                    0x2b => ',',
                    _ => {
                        let Some(text) = event.charactersIgnoringModifiers() else {
                            return original;
                        };
                        let text = text.to_string();
                        let mut chars = text.chars();
                        let Some(character) = chars.next() else {
                            return original;
                        };
                        if chars.next().is_some() {
                            return original;
                        }
                        character.to_ascii_uppercase()
                    }
                };
                let Some(shortcut) = reserved_shortcut(character, control, command, shift, option)
                else {
                    return original;
                };
                if event.r#type() == NSEventType::KeyDown {
                    // Re-entrant native callbacks must never panic on a
                    // RefCell borrow, and repeated keys cannot grow the queue.
                    let Ok(mut state) = state.try_borrow_mut() else {
                        return std::ptr::null_mut();
                    };
                    if state.shortcuts.len() < 16 {
                        state.shortcuts.push(shortcut);
                        state.changed = true;
                        drop(state);
                        wake();
                    }
                }
                std::ptr::null_mut()
            }))
            .unwrap_or(original)
        });
        // AppKit retains the block and returns the opaque monitor token.
        // Returning either the original event or null satisfies its contract.
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::KeyDown | NSEventMask::KeyUp,
                &handler,
            )
        }
        .ok_or_else(|| "browser-native-shortcuts-failed".to_owned())?;
        Ok(Self(monitor))
    }
}

impl Drop for ShortcutMonitor {
    fn drop(&mut self) {
        // This token came from addLocalMonitor and the GUI drops the native
        // page on the same main thread that created it.
        unsafe { NSEvent::removeMonitor(&self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_navigation_keys_are_distinct_from_host_tab_shortcuts() {
        for (key, action) in [('[', '<'), (']', '>'), ('R', 'R')] {
            assert_eq!(
                reserved_shortcut(key, false, true, false, false),
                Some(action)
            );
            assert_eq!(reserved_shortcut(key, false, false, false, false), None);
            assert_eq!(reserved_shortcut(key, false, true, false, true), None);
        }
        assert_eq!(reserved_shortcut('[', true, false, true, false), Some('['));
        assert_eq!(reserved_shortcut(']', true, false, true, false), Some(']'));
        assert_eq!(reserved_shortcut('L', false, true, false, false), Some('L'));
        assert_eq!(reserved_shortcut('L', true, false, false, false), Some('L'));
        assert_eq!(reserved_shortcut('C', false, true, false, false), None);
    }
}
