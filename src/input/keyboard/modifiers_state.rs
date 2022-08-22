use xkbcommon::xkb;

/// Represents the current state of the keyboard modifiers
///
/// Each field of this struct represents a modifier and is `true` if this modifier is active.
///
/// For some modifiers, this means that the key is currently pressed, others are toggled
/// (like caps lock).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModifiersState {
    /// The "control" key
    pub ctrl: bool,
    /// The "alt" key
    pub alt: bool,
    /// The "shift" key
    pub shift: bool,
    /// The "Caps lock" key
    pub caps_lock: bool,
    /// The "logo" key
    ///
    /// Also known as the "windows" key on most keyboards
    pub logo: bool,
    /// The "Num lock" key
    pub num_lock: bool,

    pub serialized: (u32, u32, u32, u32),
}

impl ModifiersState {
    pub(super) fn update_with(&mut self, state: &xkb::State) {
        self.ctrl = state.mod_name_is_active(&xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE);
        self.alt = state.mod_name_is_active(&xkb::MOD_NAME_ALT, xkb::STATE_MODS_EFFECTIVE);
        self.shift = state.mod_name_is_active(&xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_EFFECTIVE);
        self.caps_lock = state.mod_name_is_active(&xkb::MOD_NAME_CAPS, xkb::STATE_MODS_EFFECTIVE);
        self.logo = state.mod_name_is_active(&xkb::MOD_NAME_LOGO, xkb::STATE_MODS_EFFECTIVE);
        self.num_lock = state.mod_name_is_active(&xkb::MOD_NAME_NUM, xkb::STATE_MODS_EFFECTIVE);
        self.serialized = serialize_modifiers(state);
    }
}

fn serialize_modifiers(state: &xkb::State) -> (u32, u32, u32, u32) {
    let mods_depressed = state.serialize_mods(xkb::STATE_MODS_DEPRESSED);
    let mods_latched = state.serialize_mods(xkb::STATE_MODS_LATCHED);
    let mods_locked = state.serialize_mods(xkb::STATE_MODS_LOCKED);
    let layout_locked = state.serialize_layout(xkb::STATE_LAYOUT_LOCKED);

    (mods_depressed, mods_latched, mods_locked, layout_locked)
}
