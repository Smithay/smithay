/// A rectangle defined by its top-left corner and dimensions
#[derive(Copy, Clone, Debug)]
pub struct Rectangle {
    /// horizontal position of the top-leftcorner of the rectangle, in surface coordinates
    pub x: i32,
    /// vertical position of the top-leftcorner of the rectangle, in surface coordinates
    pub y: i32,
    /// width of the rectangle
    pub width: i32,
    /// height of the rectangle
    pub height: i32,
}

impl Rectangle {
    /// Checks wether given point is inside a rectangle
    pub fn contains(&self, point: (i32, i32)) -> bool {
        let (x, y) = point;
        (x >= self.x) && (x < self.x + self.width)
        && (y >= self.y) && (y < self.y + self.height)
    }
}