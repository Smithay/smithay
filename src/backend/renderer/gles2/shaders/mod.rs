/*
 * OpenGL Shaders
 */

// define constants
pub const NO_ALPHA: &str = "NO_ALPHA";
pub const EXTERNAL: &str = "EXTERNAL";
pub const DEBUG_FLAGS: &str = "DEBUG_FLAGS";

pub const VERTEX_SHADER: &str = include_str!("./texture.vert");
pub const FRAGMENT_SHADER: &str = include_str!("./texture.frag");

pub const VERTEX_SHADER_SOLID: &str = include_str!("./solid.vert");
pub const FRAGMENT_SHADER_SOLID: &str = include_str!("./solid.frag");
