//! Layout-independent modified keys, matching 1.x platform.ts::shortcutKey.
use std::collections::HashMap;

use slint::winit_030::{
    CustomApplicationHandler, EventResult,
    winit::{
        event::{ElementState, WindowEvent},
        event_loop::ActiveEventLoop,
        keyboard::{Key, KeyCode, ModifiersState, PhysicalKey},
        window::{Window, WindowId},
    },
};

#[derive(Default)]
pub struct Shortcuts {
    windows: HashMap<WindowId, WindowKeys>,
}

#[derive(Default)]
struct WindowKeys {
    modifiers: ModifiersState,
    pressed: HashMap<KeyCode, String>,
}

/// ASCII layouts keep their logical bindings (e.g. AZERTY). Non-Latin layouts
/// use the US key at the same position. Ordinary typing and AltGr stay untouched.
fn shortcut_text(physical: KeyCode, logical: &Key, modifiers: ModifiersState) -> Option<String> {
    let command = if cfg!(target_os = "macos") {
        modifiers.super_key()
    } else {
        modifiers.control_key()
    };
    if !command || modifiers.alt_key() {
        return None;
    }
    match logical {
        Key::Named(_) => return None,
        Key::Character(value) if value.len() == 1 && value.as_bytes()[0].is_ascii_graphic() => {
            return None;
        }
        _ => {}
    }
    use KeyCode::*;
    let text = match physical {
        KeyA => "a",
        KeyB => "b",
        KeyC => "c",
        KeyD => "d",
        KeyE => "e",
        KeyF => "f",
        KeyG => "g",
        KeyH => "h",
        KeyI => "i",
        KeyJ => "j",
        KeyK => "k",
        KeyL => "l",
        KeyM => "m",
        KeyN => "n",
        KeyO => "o",
        KeyP => "p",
        KeyQ => "q",
        KeyR => "r",
        KeyS => "s",
        KeyT => "t",
        KeyU => "u",
        KeyV => "v",
        KeyW => "w",
        KeyX => "x",
        KeyY => "y",
        KeyZ => "z",
        Digit0 => "0",
        Digit1 => "1",
        Digit2 => "2",
        Digit3 => "3",
        Digit4 => "4",
        Digit5 => "5",
        Digit6 => "6",
        Digit7 => "7",
        Digit8 => "8",
        Digit9 => "9",
        Backquote => "`",
        Minus => "-",
        Equal => "=",
        BracketLeft => "[",
        BracketRight => "]",
        Backslash | IntlBackslash => "\\",
        Semicolon => ";",
        Quote => "'",
        Comma => ",",
        Period => ".",
        Slash => "/",
        _ => return None,
    };
    Some(text.to_owned())
}

impl WindowKeys {
    fn rewrite(
        &mut self,
        physical: PhysicalKey,
        logical: &Key,
        state: ElementState,
    ) -> Option<String> {
        let PhysicalKey::Code(code) = physical else {
            return None;
        };
        if state == ElementState::Released {
            return self.pressed.remove(&code);
        }
        let text = shortcut_text(code, logical, self.modifiers)?;
        self.pressed.insert(code, text.clone());
        Some(text)
    }
}

impl CustomApplicationHandler for Shortcuts {
    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        window_id: WindowId,
        _winit_window: Option<&Window>,
        slint_window: Option<&slint::Window>,
        event: &WindowEvent,
    ) -> EventResult {
        match event {
            WindowEvent::ModifiersChanged(modifiers) => {
                self.windows.entry(window_id).or_default().modifiers = modifiers.state()
            }
            WindowEvent::Focused(false) | WindowEvent::Destroyed => {
                self.windows.remove(&window_id);
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => {
                if let Some(window) = slint_window
                    && let Some(text) = self.windows.entry(window_id).or_default().rewrite(
                        event.physical_key,
                        &event.logical_key,
                        event.state,
                    )
                {
                    use slint::platform::WindowEvent as Input;
                    // Slint retains the already-dispatched modifiers. Route the replacement
                    // through the whole focus tree, including text fields and the terminal.
                    window.dispatch_event(match event.state {
                        ElementState::Released => Input::KeyReleased { text: text.into() },
                        ElementState::Pressed if event.repeat => {
                            Input::KeyPressRepeated { text: text.into() }
                        }
                        ElementState::Pressed => Input::KeyPressed { text: text.into() },
                    });
                    return EventResult::PreventDefault;
                }
            }
            _ => {}
        }
        EventResult::Propagate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> ModifiersState {
        if cfg!(target_os = "macos") {
            ModifiersState::SUPER
        } else {
            ModifiersState::CONTROL
        }
    }

    #[test]
    fn non_latin_layouts_keep_shortcuts_at_the_same_keys() {
        for (physical, logical, expected) in [
            (KeyCode::KeyC, "с", "c"),
            (KeyCode::KeyV, "м", "v"),
            (KeyCode::KeyK, "л", "k"),
            (KeyCode::KeyB, "и", "b"),
            (KeyCode::KeyT, "е", "t"),
            (KeyCode::KeyW, "ц", "w"),
            (KeyCode::Comma, "б", ","),
            (KeyCode::Backquote, "ё", "`"),
            (KeyCode::KeyK, "κ", "k"),
            (KeyCode::KeyK, "ל", "k"),
            (KeyCode::KeyK, "ن", "k"),
            (KeyCode::Digit1, "١", "1"),
        ] {
            for modifiers in [command(), command() | ModifiersState::SHIFT] {
                assert_eq!(
                    shortcut_text(physical, &Key::Character(logical.into()), modifiers).as_deref(),
                    Some(expected)
                );
            }
        }
    }

    #[test]
    fn typing_ascii_layouts_altgr_and_named_keys_keep_their_native_events() {
        for logical in ["с", "é", "λ", "€"] {
            assert!(
                shortcut_text(
                    KeyCode::KeyC,
                    &Key::Character(logical.into()),
                    ModifiersState::empty()
                )
                .is_none()
            );
            assert!(
                shortcut_text(
                    KeyCode::KeyC,
                    &Key::Character(logical.into()),
                    command() | ModifiersState::ALT
                )
                .is_none()
            );
        }
        // AZERTY/QWERTZ still match the ASCII letter printed by the layout.
        assert!(shortcut_text(KeyCode::KeyQ, &Key::Character("a".into()), command()).is_none());
        assert!(shortcut_text(KeyCode::KeyY, &Key::Character("z".into()), command()).is_none());
        assert!(
            shortcut_text(
                KeyCode::F11,
                &Key::Named(slint::winit_030::winit::keyboard::NamedKey::F11),
                command()
            )
            .is_none()
        );
    }

    #[test]
    fn release_matches_the_rewritten_press_even_after_control_is_released() {
        let mut keys = WindowKeys {
            modifiers: command(),
            ..Default::default()
        };
        let physical = PhysicalKey::Code(KeyCode::KeyC);
        let logical = Key::Character("с".into());
        assert_eq!(
            keys.rewrite(physical, &logical, ElementState::Pressed)
                .as_deref(),
            Some("c")
        );
        keys.modifiers = ModifiersState::empty();
        assert_eq!(
            keys.rewrite(physical, &logical, ElementState::Released)
                .as_deref(),
            Some("c")
        );
        assert!(
            keys.rewrite(physical, &logical, ElementState::Released)
                .is_none()
        );
    }

    #[test]
    fn replacement_preserves_slint_modifiers_and_reaches_global_shortcuts() {
        use slint::{
            ComponentHandle,
            platform::{Key as SlintKey, WindowEvent as Input},
        };
        i_slint_backend_testing::init_no_event_loop();
        let ui = crate::AppWindow::new().unwrap();
        crate::i18n::init(&ui, "en");
        ui.show().unwrap();
        ui.invoke_focus_terminal();
        ui.window().dispatch_event(Input::KeyPressed {
            text: char::from(SlintKey::Control).to_string().into(),
        });
        let text = shortcut_text(KeyCode::KeyK, &Key::Character("л".into()), command()).unwrap();
        ui.window()
            .dispatch_event(Input::KeyPressed { text: text.into() });
        assert_eq!(ui.get_overlay(), "quick");
        ui.window()
            .dispatch_event(Input::KeyReleased { text: "k".into() });
        ui.window().dispatch_event(Input::KeyReleased {
            text: char::from(SlintKey::Control).to_string().into(),
        });
    }
}
