use super::settings::Config;
use crate::input::Modifiers;
use serde::Deserialize;
use std::{collections::HashMap, io};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Action {
    PreviousTab,
    NextTab,
    NewTab,
    CloseTab,
    WordLeft,
    WordRight,
    Copy,
    Paste,
    ScrollPageUp,
    ScrollPageDown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq)]
#[serde(try_from = "String")]
struct Chord {
    code: u16,
    modifiers: Modifiers,
}

impl TryFrom<String> for Chord {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let normalized = text.to_ascii_lowercase();
        let mut parts = normalized.rsplit('+').map(str::trim);
        let key = parts.next().unwrap_or_default();
        let code = match key {
            "left" => VK_LEFT,
            "right" => VK_RIGHT,
            "up" => VK_UP,
            "down" => VK_DOWN,
            "home" => VK_HOME,
            "end" => VK_END,
            "pageup" => VK_PRIOR,
            "pagedown" => VK_NEXT,
            "tab" => VK_TAB,
            "insert" => VK_INSERT,
            "delete" => VK_DELETE,
            "backspace" => VK_BACK,
            "enter" => VK_RETURN,
            "space" => VK_SPACE,
            "escape" => VK_ESCAPE,
            _ if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() => {
                u16::from(key.as_bytes()[0].to_ascii_uppercase())
            }
            _ => key
                .strip_prefix('f')
                .and_then(|n| n.parse::<u16>().ok())
                .filter(|n| (1..=12).contains(n))
                .map(|n| VK_F1 + n - 1)
                .ok_or_else(|| format!("unknown key in {text:?}"))?,
        };
        let mut modifiers = Modifiers::default();
        for part in parts {
            let modifier = match part {
                "ctrl" | "control" => &mut modifiers.control,
                "alt" => &mut modifiers.alt,
                "shift" => &mut modifiers.shift,
                _ => return Err(format!("unknown modifier {part:?} in {text:?}")),
            };
            if *modifier {
                return Err(format!("duplicate modifier {part:?} in {text:?}"));
            }
            *modifier = true;
        }
        Ok(Self { code, modifiers })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(transparent)]
pub(super) struct Keymap(HashMap<Chord, Action>);

impl Keymap {
    pub fn action(&self, code: u16, modifiers: Modifiers) -> Option<Action> {
        self.0.get(&Chord { code, modifiers }).copied()
    }
}

impl Config for Keymap {
    const STARTER: &'static str = include_str!("../../assets/keymap.json");

    fn parse(json: &str) -> io::Result<Self> {
        Ok(serde_json::from_str(json)?)
    }
}

impl Default for Keymap {
    fn default() -> Self {
        Self::parse(Self::STARTER).expect("the bundled keymap must be valid")
    }
}
