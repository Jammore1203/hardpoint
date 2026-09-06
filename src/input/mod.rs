//! Input: bindings, state tracking and the mapping from hardware to actions.
//!
//! Everything the game reads goes through `Action`, never through a raw key.
//! That is what makes rebinding work, and it is also the seam a controller
//! would slot into: a gamepad backend only has to fill in the same action
//! state and the two analogue axes.

use std::collections::HashMap;
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Action {
    MoveForward,
    MoveBack,
    MoveLeft,
    MoveRight,
    Jump,
    Crouch,
    Prone,
    Sprint,
    Fire,
    Aim,
    Reload,
    Melee,
    Lethal,
    Tactical,
    Use,
    NextWeapon,
    PrevWeapon,
    Weapon1,
    Weapon2,
    Weapon3,
    Scoreboard,
    Chat,
    TeamChat,
    Pause,
    VoiceChat,
    Respawn,
    ToggleStats,
}

pub const ALL_ACTIONS: [Action; 27] = {
    use Action::*;
    [
        MoveForward, MoveBack, MoveLeft, MoveRight, Jump, Crouch, Prone, Sprint,
        Fire, Aim, Reload, Melee, Lethal, Tactical, Use,
        NextWeapon, PrevWeapon, Weapon1, Weapon2, Weapon3,
        Scoreboard, Chat, TeamChat, Pause, Respawn, ToggleStats, VoiceChat,
    ]
};

impl Action {
    pub fn label(self) -> &'static str {
        use Action::*;
        match self {
            MoveForward => "MOVE FORWARD",
            MoveBack => "MOVE BACK",
            MoveLeft => "STRAFE LEFT",
            MoveRight => "STRAFE RIGHT",
            Jump => "JUMP",
            Crouch => "CROUCH",
            Prone => "PRONE",
            Sprint => "SPRINT",
            Fire => "FIRE",
            Aim => "AIM",
            Reload => "RELOAD",
            Melee => "MELEE",
            Lethal => "LETHAL",
            Tactical => "TACTICAL",
            Use => "USE / PLANT",
            NextWeapon => "NEXT WEAPON",
            PrevWeapon => "PREVIOUS WEAPON",
            Weapon1 => "PRIMARY",
            Weapon2 => "SIDEARM",
            Weapon3 => "MELEE WEAPON",
            Scoreboard => "SCOREBOARD",
            Chat => "CHAT",
            TeamChat => "TEAM CHAT",
            Pause => "PAUSE / BACK",
            VoiceChat => "VOICE (HOLD)",
            Respawn => "RESPAWN",
            ToggleStats => "PERFORMANCE OVERLAY",
        }
    }

    /// Config file key.
    pub fn config_key(self) -> &'static str {
        use Action::*;
        match self {
            MoveForward => "bind.forward",
            MoveBack => "bind.back",
            MoveLeft => "bind.left",
            MoveRight => "bind.right",
            Jump => "bind.jump",
            Crouch => "bind.crouch",
            Prone => "bind.prone",
            Sprint => "bind.sprint",
            Fire => "bind.fire",
            Aim => "bind.aim",
            Reload => "bind.reload",
            Melee => "bind.melee",
            Lethal => "bind.lethal",
            Tactical => "bind.tactical",
            Use => "bind.use",
            NextWeapon => "bind.nextweapon",
            PrevWeapon => "bind.prevweapon",
            Weapon1 => "bind.weapon1",
            Weapon2 => "bind.weapon2",
            Weapon3 => "bind.weapon3",
            Scoreboard => "bind.scoreboard",
            Chat => "bind.chat",
            TeamChat => "bind.teamchat",
            Pause => "bind.pause",
            VoiceChat => "bind.voice",
            Respawn => "bind.respawn",
            ToggleStats => "bind.stats",
        }
    }
}

/// A physical control a binding can point at.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    Key(KeyCode),
    Mouse(MouseButton),
    WheelUp,
    WheelDown,
    None,
}

impl Binding {
    pub fn name(self) -> String {
        match self {
            Binding::Key(k) => key_name(k).to_string(),
            Binding::Mouse(MouseButton::Left) => "MOUSE 1".into(),
            Binding::Mouse(MouseButton::Right) => "MOUSE 2".into(),
            Binding::Mouse(MouseButton::Middle) => "MOUSE 3".into(),
            Binding::Mouse(MouseButton::Back) => "MOUSE 4".into(),
            Binding::Mouse(MouseButton::Forward) => "MOUSE 5".into(),
            Binding::Mouse(MouseButton::Other(n)) => format!("MOUSE {}", n),
            Binding::WheelUp => "WHEEL UP".into(),
            Binding::WheelDown => "WHEEL DOWN".into(),
            Binding::None => "-".into(),
        }
    }

    pub fn parse(s: &str) -> Binding {
        let s = s.trim().to_ascii_uppercase();
        match s.as_str() {
            "-" | "" => Binding::None,
            "MOUSE 1" => Binding::Mouse(MouseButton::Left),
            "MOUSE 2" => Binding::Mouse(MouseButton::Right),
            "MOUSE 3" => Binding::Mouse(MouseButton::Middle),
            "MOUSE 4" => Binding::Mouse(MouseButton::Back),
            "MOUSE 5" => Binding::Mouse(MouseButton::Forward),
            "WHEEL UP" => Binding::WheelUp,
            "WHEEL DOWN" => Binding::WheelDown,
            other => key_from_name(other).map(Binding::Key).unwrap_or(Binding::None),
        }
    }
}

#[derive(Clone)]
pub struct Bindings {
    map: HashMap<Action, Binding>,
}

impl Default for Bindings {
    fn default() -> Self {
        use Action::*;
        use KeyCode as K;
        let mut map = HashMap::new();
        map.insert(MoveForward, Binding::Key(K::KeyW));
        map.insert(MoveBack, Binding::Key(K::KeyS));
        map.insert(MoveLeft, Binding::Key(K::KeyA));
        map.insert(MoveRight, Binding::Key(K::KeyD));
        map.insert(Jump, Binding::Key(K::Space));
        map.insert(Crouch, Binding::Key(K::ControlLeft));
        map.insert(Prone, Binding::Key(K::KeyZ));
        map.insert(Sprint, Binding::Key(K::ShiftLeft));
        map.insert(Fire, Binding::Mouse(MouseButton::Left));
        map.insert(Aim, Binding::Mouse(MouseButton::Right));
        map.insert(Reload, Binding::Key(K::KeyR));
        map.insert(Melee, Binding::Key(K::KeyV));
        map.insert(Lethal, Binding::Key(K::KeyG));
        map.insert(Tactical, Binding::Key(K::KeyF));
        map.insert(Use, Binding::Key(K::KeyE));
        map.insert(NextWeapon, Binding::WheelUp);
        map.insert(PrevWeapon, Binding::WheelDown);
        map.insert(Weapon1, Binding::Key(K::Digit1));
        map.insert(Weapon2, Binding::Key(K::Digit2));
        map.insert(Weapon3, Binding::Key(K::Digit3));
        map.insert(Scoreboard, Binding::Key(K::Tab));
        map.insert(Chat, Binding::Key(K::KeyT));
        map.insert(TeamChat, Binding::Key(K::KeyY));
        map.insert(Pause, Binding::Key(K::Escape));
        map.insert(VoiceChat, Binding::Key(K::KeyB));
        map.insert(Respawn, Binding::Key(K::Space));
        map.insert(ToggleStats, Binding::Key(K::F3));
        Bindings { map }
    }
}

impl Bindings {
    pub fn get(&self, a: Action) -> Binding {
        self.map.get(&a).copied().unwrap_or(Binding::None)
    }
    pub fn set(&mut self, a: Action, b: Binding) {
        // A control may only drive one action, so rebinding steals it.
        if b != Binding::None {
            let clashing: Vec<Action> = self.map.iter()
                .filter(|(k, v)| **v == b && **k != a)
                .map(|(k, _)| *k)
                .collect();
            for c in clashing { self.map.insert(c, Binding::None); }
        }
        self.map.insert(a, b);
    }
    /// Stores a binding without the steal rule. Loading a saved file must not
    /// re-run conflict resolution: jump and respawn deliberately share a key,
    /// and `set` would strip one of them on every launch.
    pub fn assign(&mut self, a: Action, b: Binding) { self.map.insert(a, b); }

    pub fn reset(&mut self) { *self = Bindings::default(); }
}

/// Live input state, updated from window events.
pub struct InputState {
    keys: [bool; 256],
    keys_prev: [bool; 256],
    mouse: [bool; 8],
    mouse_prev: [bool; 8],
    wheel: f32,
    wheel_consumed: bool,
    /// Accumulated raw mouse motion since the last frame.
    pub mouse_dx: f32,
    pub mouse_dy: f32,
    pub cursor: (f32, f32),
    pub cursor_moved: bool,
    /// Characters typed this frame, for text entry.
    pub typed: String,
    pub backspace: bool,
    pub enter: bool,
    pub escape: bool,
    /// Set while the game is capturing the pointer for look control.
    pub captured: bool,
    /// The last control pressed, used by the rebinding screen.
    pub last_binding: Option<Binding>,
}

impl Default for InputState {
    fn default() -> Self { InputState::new() }
}

impl InputState {
    pub fn new() -> InputState {
        InputState {
            keys: [false; 256],
            keys_prev: [false; 256],
            mouse: [false; 8],
            mouse_prev: [false; 8],
            wheel: 0.0,
            wheel_consumed: false,
            mouse_dx: 0.0,
            mouse_dy: 0.0,
            cursor: (0.0, 0.0),
            cursor_moved: false,
            typed: String::new(),
            backspace: false,
            enter: false,
            escape: false,
            captured: false,
            last_binding: None,
        }
    }

    /// Called at the end of every frame.
    pub fn end_frame(&mut self) {
        self.keys_prev = self.keys;
        self.mouse_prev = self.mouse;
        self.mouse_dx = 0.0;
        self.mouse_dy = 0.0;
        self.wheel = 0.0;
        self.wheel_consumed = false;
        self.typed.clear();
        self.backspace = false;
        self.enter = false;
        self.escape = false;
        self.cursor_moved = false;
        self.last_binding = None;
    }

    pub fn set_key(&mut self, key: KeyCode, down: bool) {
        let i = key_index(key);
        if i < self.keys.len() {
            if down && !self.keys[i] { self.last_binding = Some(Binding::Key(key)); }
            self.keys[i] = down;
        }
    }

    pub fn set_mouse(&mut self, button: MouseButton, down: bool) {
        let i = mouse_index(button);
        if i < self.mouse.len() {
            if down && !self.mouse[i] { self.last_binding = Some(Binding::Mouse(button)); }
            self.mouse[i] = down;
        }
    }

    pub fn add_wheel(&mut self, delta: f32) {
        self.wheel += delta;
        if delta > 0.0 { self.last_binding = Some(Binding::WheelUp); }
        else if delta < 0.0 { self.last_binding = Some(Binding::WheelDown); }
    }

    pub fn add_motion(&mut self, dx: f32, dy: f32) {
        self.mouse_dx += dx;
        self.mouse_dy += dy;
    }

    pub fn key_down(&self, key: KeyCode) -> bool {
        let i = key_index(key);
        i < self.keys.len() && self.keys[i]
    }

    pub fn key_pressed(&self, key: KeyCode) -> bool {
        let i = key_index(key);
        i < self.keys.len() && self.keys[i] && !self.keys_prev[i]
    }

    pub fn mouse_down(&self, button: MouseButton) -> bool {
        let i = mouse_index(button);
        i < self.mouse.len() && self.mouse[i]
    }

    pub fn mouse_pressed(&self, button: MouseButton) -> bool {
        let i = mouse_index(button);
        i < self.mouse.len() && self.mouse[i] && !self.mouse_prev[i]
    }

    pub fn wheel(&self) -> f32 { self.wheel }

    // -------------------------------------------------------------- actions

    pub fn held(&self, b: &Bindings, a: Action) -> bool {
        match b.get(a) {
            Binding::Key(k) => self.key_down(k),
            Binding::Mouse(m) => self.mouse_down(m),
            Binding::WheelUp => self.wheel > 0.0,
            Binding::WheelDown => self.wheel < 0.0,
            Binding::None => false,
        }
    }

    pub fn pressed(&self, b: &Bindings, a: Action) -> bool {
        match b.get(a) {
            Binding::Key(k) => self.key_pressed(k),
            Binding::Mouse(m) => self.mouse_pressed(m),
            Binding::WheelUp => self.wheel > 0.0,
            Binding::WheelDown => self.wheel < 0.0,
            Binding::None => false,
        }
    }

    /// Swallows the current press of `a`, so a later reader this frame does
    /// not see the same edge.
    ///
    /// A frame runs the world before it runs the interface. Without this, one
    /// tap of Escape opened the pause menu during the update and the pause
    /// menu's own back-handler closed it again during the draw, and the key
    /// looked dead.
    pub fn consume(&mut self, b: &Bindings, a: Action) {
        match b.get(a) {
            Binding::Key(k) => {
                let i = key_index(k);
                if i < self.keys.len() { self.keys_prev[i] = self.keys[i]; }
                if k == KeyCode::Escape { self.escape = false; }
            }
            Binding::Mouse(m) => {
                let i = mouse_index(m);
                if i < self.mouse.len() { self.mouse_prev[i] = self.mouse[i]; }
            }
            Binding::WheelUp | Binding::WheelDown => { self.wheel = 0.0; }
            Binding::None => {}
        }
    }

    /// The same, for the bare Escape edge the menus read directly.
    pub fn consume_escape(&mut self) {
        self.escape = false;
        let i = key_index(KeyCode::Escape);
        if i < self.keys.len() { self.keys_prev[i] = self.keys[i]; }
    }
}

/// Dense index for a key code, so state fits in a fixed array.
fn key_index(k: KeyCode) -> usize {
    // The discriminant is stable within a winit version and we only need a
    // dense-enough mapping; anything beyond the table falls into a shared
    // bucket which is harmless for keys the game never reads.
    (key_ordinal(k) as usize) & 0xFF
}

fn mouse_index(b: MouseButton) -> usize {
    match b {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
        MouseButton::Other(n) => 5 + (n as usize % 3),
    }
}

macro_rules! key_table {
    ($($code:ident => $ord:expr, $name:expr;)*) => {
        fn key_ordinal(k: KeyCode) -> u16 {
            match k { $(KeyCode::$code => $ord,)* _ => 255 }
        }
        pub fn key_name(k: KeyCode) -> &'static str {
            match k { $(KeyCode::$code => $name,)* _ => "?" }
        }
        pub fn key_from_name(s: &str) -> Option<KeyCode> {
            match s { $($name => Some(KeyCode::$code),)* _ => None }
        }
    };
}

key_table! {
    KeyA => 1, "A"; KeyB => 2, "B"; KeyC => 3, "C"; KeyD => 4, "D";
    KeyE => 5, "E"; KeyF => 6, "F"; KeyG => 7, "G"; KeyH => 8, "H";
    KeyI => 9, "I"; KeyJ => 10, "J"; KeyK => 11, "K"; KeyL => 12, "L";
    KeyM => 13, "M"; KeyN => 14, "N"; KeyO => 15, "O"; KeyP => 16, "P";
    KeyQ => 17, "Q"; KeyR => 18, "R"; KeyS => 19, "S"; KeyT => 20, "T";
    KeyU => 21, "U"; KeyV => 22, "V"; KeyW => 23, "W"; KeyX => 24, "X";
    KeyY => 25, "Y"; KeyZ => 26, "Z";
    Digit0 => 27, "0"; Digit1 => 28, "1"; Digit2 => 29, "2"; Digit3 => 30, "3";
    Digit4 => 31, "4"; Digit5 => 32, "5"; Digit6 => 33, "6"; Digit7 => 34, "7";
    Digit8 => 35, "8"; Digit9 => 36, "9";
    Space => 37, "SPACE"; Enter => 38, "ENTER"; Escape => 39, "ESC";
    Tab => 40, "TAB"; Backspace => 41, "BACKSPACE";
    ShiftLeft => 42, "LSHIFT"; ShiftRight => 43, "RSHIFT";
    ControlLeft => 44, "LCTRL"; ControlRight => 45, "RCTRL";
    AltLeft => 46, "LALT"; AltRight => 47, "RALT";
    ArrowUp => 48, "UP"; ArrowDown => 49, "DOWN";
    ArrowLeft => 50, "LEFT"; ArrowRight => 51, "RIGHT";
    F1 => 52, "F1"; F2 => 53, "F2"; F3 => 54, "F3"; F4 => 55, "F4";
    F5 => 56, "F5"; F6 => 57, "F6"; F7 => 58, "F7"; F8 => 59, "F8";
    F9 => 60, "F9"; F10 => 61, "F10"; F11 => 62, "F11"; F12 => 63, "F12";
    Minus => 64, "-"; Equal => 65, "="; BracketLeft => 66, "["; BracketRight => 67, "]";
    Semicolon => 68, ";"; Quote => 69, "'"; Backquote => 70, "`";
    Backslash => 71, "\\"; Comma => 72, ","; Period => 73, "."; Slash => 74, "/";
    CapsLock => 75, "CAPS"; Insert => 76, "INS"; Delete => 77, "DEL";
    Home => 78, "HOME"; End => 79, "END"; PageUp => 80, "PGUP"; PageDown => 81, "PGDN";
    Numpad0 => 82, "NUM0"; Numpad1 => 83, "NUM1"; Numpad2 => 84, "NUM2";
    Numpad3 => 85, "NUM3"; Numpad4 => 86, "NUM4"; Numpad5 => 87, "NUM5";
    Numpad6 => 88, "NUM6"; Numpad7 => 89, "NUM7"; Numpad8 => 90, "NUM8";
    Numpad9 => 91, "NUM9"; NumpadEnter => 92, "NUMENTER";
}
