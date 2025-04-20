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
    /// The "ISO level 3 shift" key
    ///
    /// Also known as the "AltGr" key
    pub iso_level3_shift: bool,

    /// The "ISO level 5 shift" key
    pub iso_level5_shift: bool,

    /// Serialized modifier state, as send e.g. by the wl_keyboard protocol
    pub serialized: SerializedMods,
}

impl ModifiersState {
    /// Update the modifiers state from an xkb state
    pub fn update_with(&mut self, state: &xkb::State) {
        self.ctrl = state.mod_name_is_active(&xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE);
        self.alt = state.mod_name_is_active(&xkb::MOD_NAME_ALT, xkb::STATE_MODS_EFFECTIVE);
        self.shift = state.mod_name_is_active(&xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_EFFECTIVE);
        self.caps_lock = state.mod_name_is_active(&xkb::MOD_NAME_CAPS, xkb::STATE_MODS_EFFECTIVE);
        self.logo = state.mod_name_is_active(&xkb::MOD_NAME_LOGO, xkb::STATE_MODS_EFFECTIVE);
        self.num_lock = state.mod_name_is_active(&xkb::MOD_NAME_NUM, xkb::STATE_MODS_EFFECTIVE);
        self.iso_level3_shift =
            state.mod_name_is_active(&xkb::MOD_NAME_ISO_LEVEL3_SHIFT, xkb::STATE_MODS_EFFECTIVE);
        self.iso_level5_shift = state.mod_name_is_active(&xkb::MOD_NAME_MOD3, xkb::STATE_MODS_EFFECTIVE);
        self.serialized = serialize_modifiers(state);
    }

    /// Serialize modifier state back to be sent to xkb.
    pub fn serialize_back(&self, state: &xkb::State) -> SerializedMods {
        let keymap = state.get_keymap();

        let mut locked: u32 = 0;
        let mut depressed: u32 = 0;

        if self.caps_lock {
            let index = keymap.mod_get_index(&xkb::MOD_NAME_CAPS);
            if index != xkb::MOD_INVALID {
                locked |= 1 << index;
            }
        }
        if self.num_lock {
            let index = keymap.mod_get_index(&xkb::MOD_NAME_NUM);
            if index != xkb::MOD_INVALID {
                locked |= 1 << index;
            }
        }
        if self.ctrl {
            let index = keymap.mod_get_index(&xkb::MOD_NAME_CTRL);
            if index != xkb::MOD_INVALID {
                depressed |= 1 << index;
            }
        }
        if self.alt {
            let index = keymap.mod_get_index(&xkb::MOD_NAME_ALT);
            if index != xkb::MOD_INVALID {
                depressed |= 1 << index;
            }
        }
        if self.shift {
            let index = keymap.mod_get_index(&xkb::MOD_NAME_SHIFT);
            if index != xkb::MOD_INVALID {
                depressed |= 1 << index;
            }
        }
        if self.logo {
            let index = keymap.mod_get_index(&xkb::MOD_NAME_LOGO);
            if index != xkb::MOD_INVALID {
                depressed |= 1 << index;
            }
        }
        if self.iso_level3_shift {
            let index = keymap.mod_get_index(&xkb::MOD_NAME_ISO_LEVEL3_SHIFT);
            if index != xkb::MOD_INVALID {
                depressed |= 1 << index;
            }
        }
        if self.iso_level5_shift {
            let index = keymap.mod_get_index(&xkb::MOD_NAME_MOD3);
            if index != xkb::MOD_INVALID {
                depressed |= 1 << index;
            }
        }

        let layout_effective = state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE);

        SerializedMods {
            depressed,
            latched: 0,
            locked,
            layout_effective,
        }
    }
}

/// Serialized modifier state
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SerializedMods {
    /// Depressed modifiers
    pub depressed: u32,
    /// Latched modifiers
    pub latched: u32,
    /// Locked modifiers
    pub locked: u32,
    /// Effective keyboard layout
    pub layout_effective: u32,
}

fn serialize_modifiers(state: &xkb::State) -> SerializedMods {
    let depressed = state.serialize_mods(xkb::STATE_MODS_DEPRESSED);
    let latched = state.serialize_mods(xkb::STATE_MODS_LATCHED);
    let locked = state.serialize_mods(xkb::STATE_MODS_LOCKED);
    let layout_effective = state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE);

    SerializedMods {
        depressed,
        latched,
        locked,
        layout_effective,
    }
}
