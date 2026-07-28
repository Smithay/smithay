/// The Motif WM Hints
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MwmHints {
    /// Lists the actions that can be done on the window.
    ///
    /// If `None`, all actions are assumed available.
    pub functions: Option<MwmFunctionsHint>,

    /// List decorations (if any) that the app has requested be shown.
    ///
    /// If `None`, all decorations should be shown.
    pub decorations: Option<MwmDecorationsHint>,

    /// The app's requested input mode.
    pub input_mode: Option<MwmInputMode>,

    /// The app's requested window status (type).
    pub status: Option<MwmStatusHint>,
}

bitflags::bitflags! {
    /// Functions available for the window.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MwmFunctionsHint: u32 {
        /// All functions available, *except* for those specified by other present bits.
        const ALL = (1 << 0);
        /// The window can be resized.
        const RESIZE = (1 << 1);
        /// The window can be moved.
        const MOVE = (1 << 2);
        /// The window can be hidden.
        const MINIMIZE = (1 << 3);
        /// The window can be maximized.
        const MAXIMIZE = (1 << 4);
        /// The window can be closed.
        const CLOSE = (1 << 5);
    }
}

bitflags::bitflags! {
    /// Decoration parts the app would like shown.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MwmDecorationsHint: u32 {
        /// All decoration parts should be shown, *except* for those specified by other present bits.
        const ALL = (1 << 0);
        /// A border/frame should be drawn around the window.
        const BORDER = (1 << 1);
        /// Resize handles should be shown.
        const RESIZEH = (1 << 2);
        /// The window's title bar should be displayed.
        const TITLE = (1 << 3);
        /// A window menu button should be shown.
        const MENU = (1 << 4);
        /// A minimize button should be shown.
        const MINIMIZE = (1 << 5);
        /// A maximize button should be shown.
        const MAXIMIZE = (1 << 6);
    }
}

/// The requested input mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum MwmInputMode {
    /// Input goes to any window.
    Modeless = 0,
    /// Input does not go to ancestors of this window.
    PrimaryApplicationModal = 1,
    /// Input goes only to this window.
    SystemModal = 2,
    /// Input does not go to other windows in this application.
    FullApplicationModal = 3,
}

bitflags::bitflags! {
    /// Other status information about the window.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MwmStatusHint: u32 {
        /// The window was "torn off" another window.
        const TEAROFF_WINDOW = (1 << 0);
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct MwmHintsFlags: u32 {
        const FUNCTIONS = (1 << 0);
        const DECORATIONS = (1 << 1);
        const INPUT_MODE = (1 << 2);
        const STATUS = (1 << 3);
    }
}

impl TryFrom<u32> for MwmInputMode {
    type Error = std::io::Error;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Modeless),
            1 => Ok(Self::PrimaryApplicationModal),
            2 => Ok(Self::SystemModal),
            3 => Ok(Self::FullApplicationModal),
            other => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{other} is not valid for MWM input mode"),
            )),
        }
    }
}

impl MwmHints {
    pub(super) fn parse(data: &[u32]) -> Option<MwmHints> {
        match data {
            [flags, functions, decorations, input_mode, status] => {
                let flags = MwmHintsFlags::from_bits(*flags)?;
                let functions = flags
                    .contains(MwmHintsFlags::FUNCTIONS)
                    .then(|| MwmFunctionsHint::from_bits_truncate(*functions));
                let decorations = flags
                    .contains(MwmHintsFlags::DECORATIONS)
                    .then(|| MwmDecorationsHint::from_bits_truncate(*decorations));
                let input_mode = flags
                    .contains(MwmHintsFlags::INPUT_MODE)
                    .then(|| MwmInputMode::try_from(*input_mode).ok())
                    .flatten();
                let status = flags
                    .contains(MwmHintsFlags::STATUS)
                    .then(|| MwmStatusHint::from_bits_truncate(*status));

                Some(Self {
                    functions,
                    decorations,
                    input_mode,
                    status,
                })
            }
            _ => None,
        }
    }
}
