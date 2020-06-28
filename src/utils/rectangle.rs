/// A rectangle defined by its top-left corner and dimensions
#[derive(Copy, Clone, Debug, Default)]
pub struct Rectangle {
    /// horizontal position of the top-left corner of the rectangle, in surface coordinates
    pub x: i32,
    /// vertical position of the top-left corner of the rectangle, in surface coordinates
    pub y: i32,
    /// width of the rectangle
    pub width: i32,
    /// height of the rectangle
    pub height: i32,
}

impl Rectangle {
    /// Checks whether given point is inside a rectangle
    pub fn contains(&self, point: (i32, i32)) -> bool {
        let (x, y) = point;
        (x >= self.x) && (x < self.x + self.width) && (y >= self.y) && (y < self.y + self.height)
    }

    /// Checks whether a given rectangle overlaps with this one
    pub fn overlaps(&self, other: &Rectangle) -> bool {
        // if the rectangle is not outside of the other
        // they must overlap
        !(
            // self is left of other
            self.x + self.width < other.x
            // self is right of other
            ||  self.x > other.x + other.width
            // self is above of other
            ||  self.y + self.height < other.y
            // self is below of other
            ||  self.y > other.y + other.height
        )
    }
}
