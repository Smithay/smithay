/// Functions to render cursors on graphics backend independently from it's rendering techique.
///
/// In the most cases this will be the fastest available implementation utilizing hardware composing
/// where possible. This may however be quite restrictive in terms of supported formats.
///
/// For those reasons you may always choose to render your cursor(s) (partially) in software instead.
pub trait CursorBackend {
    /// Format representing the image drawn for the cursor.
    type CursorFormat: ?Sized;

    /// Error the underlying backend throws if operations fail
    type Error: 'static;

    /// Sets the cursor position and therefore updates the drawn cursors position.
    /// Useful as well for e.g. pointer wrapping.
    ///
    /// Not guaranteed to be supported on every backend. The result usually
    /// depends on the backend, the cursor might be "owned" by another more privileged
    /// compositor (running nested).
    ///
    /// In these cases setting the position is actually not required, as movement is done
    /// by the higher compositor and not by the backend. It is still good practice to update
    /// the position after every recieved event, but don't rely on pointer wrapping working.
    ///
    fn set_cursor_position(&self, x: u32, y: u32) -> Result<(), Self::Error>;

    /// Set the cursor drawn on the [`CursorBackend`].
    ///
    /// The format is entirely dictated by the concrete implementation and might range
    /// from raw image buffers over a fixed list of possible cursor types to simply the
    /// void type () to represent no possible customization of the cursor itself.
    fn set_cursor_representation(
        &self,
        cursor: &Self::CursorFormat,
        hotspot: (u32, u32),
    ) -> Result<(), Self::Error>;

    /// Clear the current cursor image drawn on the [`CursorBackend`].
    fn clear_cursor_representation(&self) -> Result<(), Self::Error>;
}
