use std::fs::File;
pub use xkbcommon::xkb;

/// Configuration for xkbcommon.
///
/// For the fields that are not set ("" or None, as set in the `Default` impl), xkbcommon will use
/// the values from the environment variables `XKB_DEFAULT_RULES`, `XKB_DEFAULT_MODEL`,
/// `XKB_DEFAULT_LAYOUT`, `XKB_DEFAULT_VARIANT` and `XKB_DEFAULT_OPTIONS`.
///
/// For details, see the [documentation at xkbcommon.org][docs].
///
/// [docs]: https://xkbcommon.org/doc/current/structxkb__rule__names.html
#[derive(Clone, Debug, Default)]
pub struct XkbConfig<'a> {
    /// The rules file to use.
    ///
    /// The rules file describes how to interpret the values of the model, layout, variant and
    /// options fields.
    pub rules: &'a str,
    /// The keyboard model by which to interpret keycodes and LEDs.
    pub model: &'a str,
    /// A comma separated list of layouts (languages) to include in the keymap.
    pub layout: &'a str,
    /// A comma separated list of variants, one per layout, which may modify or augment the
    /// respective layout in various ways.
    pub variant: &'a str,
    /// A comma separated list of options, through which the user specifies non-layout related
    /// preferences, like which key combinations are used for switching layouts, or which key is the
    /// Compose key.
    pub options: Option<String>,
    /// Path to a file from which the keymap can be compiled. Allows the user to provide a
    /// stand-alone keymap file that will be used instead of a system keymap.
    pub file: Option<String>,
}

impl<'a> XkbConfig<'a> {
    pub(crate) fn compile_keymap(&self, context: &xkb::Context) -> Result<xkb::Keymap, ()> {
        match &self.file {
            Some(f) => {
                let mut file = File::open(f).map_err(|_| ())?;
                xkb::Keymap::new_from_file(
                    context,
                    &mut file,
                    xkb::KEYMAP_FORMAT_TEXT_V1,
                    xkb::KEYMAP_COMPILE_NO_FLAGS,
                )
                .ok_or(())
            }
            None => xkb::Keymap::new_from_names(
                context,
                self.rules,
                self.model,
                self.layout,
                self.variant,
                self.options.clone(),
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
            .ok_or(()),
        }
    }
}
