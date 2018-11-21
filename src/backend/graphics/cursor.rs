/// Functions to render cursors on any graphics backend independently from it's rendering techique.
pub trait CursorBackend<'a> {
    /// Format representing the image drawn for the cursor.
    type CursorFormat: 'a;

    /// Error the underlying backend throws if operations fail
    type Error;

    /// Sets the cursor position and therefor updates the drawn cursors position.
    /// Useful as well for e.g. pointer wrapping.
    ///
    /// Not guaranteed to be supported on every backend. The result usually
    /// depends on the backend, the cursor might be "owned" by another more priviledged
    /// compositor (running nested).
    ///
    /// In these cases setting the position is actually not required, as movement is done
    /// by the higher compositor and not by the backend. It is still good practice to update
    /// the position after every recieved event, but don't rely on pointer wrapping working.
    ///
    fn set_cursor_position(&self, x: u32, y: u32) -> Result<(), Self::Error>;

    /// Set the cursor drawn on the `CursorBackend`.
    ///
    /// The format is entirely dictated by the concrete implementation and might range
    /// from raw image buffers over a fixed list of possible cursor types to simply the
    /// void type () to represent no possible customization of the cursor itself.
    fn set_cursor_representation<'b>(
        &'b self,
        cursor: Self::CursorFormat,
        hotspot: (u32, u32),
    ) -> Result<(), Self::Error>
    where 'a: 'b;
}